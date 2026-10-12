//! The state of one layout pass (the painter) and the pieces of inline content it collects.

use super::*;

/// Horizontal room of a block: its left edge and width.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Area {
    pub(crate) x: i32,
    pub(crate) w: i32,
}

/// How a run of text looks.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Style {
    pub(crate) font: Font,
    pub(crate) color: Rgb,
    pub(crate) deco: u8,
    pub(crate) bg: Option<Rgb>,
    pub(crate) link: Option<usize>,
    /// Line-height of the run in pixels.
    pub(crate) lh: i32,
    /// Pixels the run is raised above the baseline (negative: lowered).
    pub(crate) shift: i32,
}

/// An inline object: sized when its line is laid out.
pub(crate) enum Obj {
    Img {
        idx: usize,
        /// Declared `width` / `height` attributes (0 = absent), already zoomed.
        dw: i32,
        dh: i32,
        state: ImgState,
        alt: String,
    },
    Field {
        form: usize,
        field: usize,
        kind: FieldKind,
        /// Width in characters (text controls).
        chars: usize,
        /// Visible lines (`<textarea>`).
        rows: usize,
        label: String,
    },
}

/// One piece of inline content waiting for its line.
pub(crate) enum Item {
    Word {
        text: String,
        st: Style,
        /// A collapsible space precedes the word; the font is that of the text holding it.
        sp: Option<Font>,
        /// May break between any two characters (CJK, preformatted lines).
        any: bool,
        /// Must stay on the line of the previous item (`white-space: nowrap`).
        nb: bool,
    },
    Obj {
        obj: Obj,
        st: Style,
        sp: Option<Font>,
    },
    Br(Style),
    /// Horizontal padding of an inline box (its background shows through it).
    Gap(i32, Style, Option<Font>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkerKind {
    Bullet(ListStyle),
    Number,
}

/// A list marker waiting for the first line of its item.
pub(crate) struct Marker {
    pub(crate) kind: MarkerKind,
    pub(crate) text: String,
    pub(crate) color: Rgb,
    /// Right edge of the marker (page x).
    pub(crate) right: i32,
}

pub(crate) struct ListCtx {
    pub(crate) ordered: bool,
    pub(crate) next: i32,
}

pub(crate) struct Painter<'a> {
    pub(crate) cmds: Vec<Cmd>,
    pub(crate) links: Vec<String>,
    pub(crate) hits: Vec<LinkHit>,
    pub(crate) images: Vec<ImgRef>,
    pub(crate) forms: Vec<FormInfo>,
    pub(crate) fields: Vec<FieldBox>,
    /// The `<form>` being laid out, if any.
    pub(crate) cur_form: Option<usize>,
    /// Zoom in percent.
    pub(crate) zoom: i32,
    pub(crate) lookup: &'a dyn ImageLookup,
    pub(crate) m: &'a dyn TextMetrics,
    pub(crate) sheet: &'a Stylesheet,
    /// Ancestors of the element being styled, outermost first.
    pub(crate) anc: Vec<&'a Element>,
    pub(crate) budget: u32,
    /// Characters of text emitted so far.
    pub(crate) chars: usize,
    /// Inline content waiting for its line boxes.
    pub(crate) items: Vec<Item>,
    /// An empty item list with room kept, swapped in while a flush reads the real one.
    pub(crate) spare_items: Vec<Item>,
    /// What the metrics answered for the fonts seen so far: `(font, space q8, ascent, line)`.
    /// A page uses a handful of fonts and asks for these once per word.
    pub(crate) fm: core::cell::RefCell<Vec<(Font, i32, i32, i32)>>,
    /// A collapsible space precedes the next word (and the font of the text it came from).
    pub(crate) space: Option<Font>,
    /// The `<a href>` the content belongs to.
    pub(crate) link: Option<usize>,
    /// Vertical margin waiting at the current y (it collapses with the next one).
    pub(crate) pending: i32,
    pub(crate) marker: Option<Marker>,
    pub(crate) lists: Vec<ListCtx>,
    /// `html` / `body` background.
    pub(crate) canvas: Option<Rgb>,
    /// Indices of the background and border commands of the block laid out last (the table
    /// code stretches a cell's box to its row).
    pub(crate) last_box: (Option<usize>, Option<usize>),
    /// Floats that still have lines beside them, in page coordinates.
    pub(crate) floats: Vec<FloatBox>,
    /// The page has no doctype: tables do not inherit `text-align` from their surroundings.
    pub(crate) quirks: bool,
}

impl Painter<'_> {
    /// Natural line height of `f`, sanitised: the metrics are the caller's, and layout must
    /// stay in range whatever they answer.
    pub(crate) fn nat(&self, f: Font) -> i32 {
        self.metrics_of(f).2
    }

    /// Ascent of `f`, sanitised like [`Painter::nat`].
    pub(crate) fn asc(&self, f: Font) -> i32 {
        self.metrics_of(f).1
    }

    /// Width of one space in `f`, q8.
    pub(crate) fn space_q8(&self, f: Font) -> i32 {
        self.metrics_of(f).0
    }

    /// `(space q8, ascent, line height)` of `f`, asked of the metrics once.
    pub(crate) fn metrics_of(&self, f: Font) -> (i32, i32, i32) {
        let mut c = self.fm.borrow_mut();
        if let Some(e) = c.iter().find(|e| e.0 == f) {
            return (e.1, e.2, e.3);
        }
        let v = (
            self.m.width_q8(" ", f).clamp(0, 1 << 26),
            self.m.ascent(f).clamp(0, 1 << 16),
            self.m.line_height(f).clamp(1, 1 << 16),
        );
        if c.len() < 32 {
            c.push((f, v.0, v.1, v.2));
        }
        v
    }

    /// Scale a length by the zoom (integer arithmetic).
    pub(crate) fn z(&self, v: i32) -> i32 {
        zoom_px(v, self.zoom)
    }

    pub(crate) fn font(&self, c: &Computed) -> Font {
        Font {
            size: self.z(c.font_px).clamp(4, 600) as u16,
            bold: c.bold,
            italic: c.italic,
            mono: c.mono,
        }
    }

    pub(crate) fn lh(&self, c: &Computed, font: Font) -> i32 {
        match c.line_h {
            LineH::Normal => self.nat(font),
            LineH::Mult(pct) => (i32::from(font.size) * pct + 50) / 100,
            LineH::Px(px) => self.z(px),
        }
        .clamp(1, 4000)
    }

    pub(crate) fn style(&self, c: &Computed) -> Style {
        let font = self.font(c);
        Style {
            font,
            color: c.color,
            deco: (if c.underline { DECO_UNDERLINE } else { 0 })
                | (if c.strike { DECO_STRIKE } else { 0 }),
            // Only an inline element paints a background behind its words; a block paints its own box.
            bg: c.bg.filter(|_| c.display == Disp::Inline),
            link: self.link,
            lh: self.lh(c, font),
            shift: i32::from(c.vshift) * (i32::from(font.size) * 2 / 5).max(1),
        }
    }

    pub(crate) fn push_cmd(&mut self, cmd: Cmd) {
        let low = match &cmd {
            Cmd::Rect { y, .. }
            | Cmd::Border { y, .. }
            | Cmd::Text { y, .. }
            | Cmd::Image { y, .. } => *y,
        };
        if self.cmds.len() < MAX_CMDS && low < MAX_PAGE_H {
            self.cmds.push(cmd);
        }
    }
}

// ---- text helpers ----

pub(crate) const Q: i32 = 256;

// ---- inline layout ----

/// One thing placed on a line.
pub(crate) struct Piece {
    pub(crate) kind: PieceKind,
    pub(crate) st: Style,
    /// Left edge from the line start, q8 pixels.
    pub(crate) x: i32,
    /// Width, q8 pixels.
    pub(crate) w: i32,
}

pub(crate) enum PieceKind {
    Text(String),
    /// An object: its box and how far it hangs below the baseline.
    Obj(Obj, i32, i32, i32),
    /// Inline padding.
    Gap,
}

pub(crate) struct Line {
    pub(crate) pieces: Vec<Piece>,
    /// Width so far, q8.
    pub(crate) w: i32,
    pub(crate) top: i32,
    pub(crate) bottom: i32,
}

// ---- objects: pictures and controls ----

/// Font size of the text inside controls (CSS px).
pub(crate) const CONTROL_PX: i32 = 14;

// ---- flow: blocks and inline content ----
