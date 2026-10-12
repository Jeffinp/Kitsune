//! Block layout with inline text flow, producing the display list.
//!
//! Text is measured, never counted: every width comes from the caller's
//! [`TextMetrics`], so words wrap at their real widths whatever the face. Line
//! boxes follow the CSS model closely enough for ordinary pages: a strut from the
//! block's own font, per-item baselines, `line-height`, `text-align`, collapsing
//! vertical margins, `width`/`max-width` with auto margins, borders, rounded
//! corners and backgrounds, lists with markers, tables, preformatted text.
//!
//! Besides text and boxes the layout places two kinds of *inline objects* in the
//! line boxes: pictures (`<img>`, see [`super::imgcache`]) and form controls
//! (`<input>`, `<button>`, see [`super::form`]). Both are sized here (declared
//! attributes first, then what is known about the picture), so a page lays out the
//! same before and after its images arrive unless a picture's own size differs from
//! what the page declared.
//!
//! Integer zoom (percent) scales every length and the font sizes; no floating
//! point anywhere. Everything a page controls is bounded: nodes, rules, depth,
//! words, characters, commands.

use super::Rgb;
use super::css::{MAX_RULES, Stylesheet, parse_css};
use super::dom::{
    Element, Node, decode_attr, fold_display, is_html_space, parse_html, text_content,
};
use super::form::{FieldInfo, FieldKind, FormInfo};
use super::imgcache::{ImageLookup, ImgState, NoImages};
use super::metrics::{FixedAdvance, Font, TextMetrics};
use super::style::{
    Align, Clear, Computed, CrossAlign, Disp, Float, Justify, Len, LineH, ListStyle, Pos, Track,
    UA_CSS, Ws, compute, compute_pseudo, transform_text, zoom_px,
};
use alloc::string::String;
use alloc::vec::Vec;

mod flex;
mod flow;
mod inline;
mod objects;
mod text;

mod doc;
mod painter;
pub use doc::*;
pub(crate) use flex::*;
pub(crate) use flow::*;
use inline::*;
pub(super) use objects::*;
pub(crate) use painter::*;
use text::*;
mod table;

// ---- the display list ----
/// Underline bit of [`Cmd::Text::deco`].
pub const DECO_UNDERLINE: u8 = 1;
/// Line-through bit of [`Cmd::Text::deco`].
pub const DECO_STRIKE: u8 = 2;
/// A rectangle, a run of text or a picture to paint.
#[derive(Debug, Clone)]
pub enum Cmd {
    /// A filled rectangle with rounded corners (`radius` 0 = square).
    Rect {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        color: Rgb,
        radius: i32,
    },
    /// A border drawn inside the box: widths top, right, bottom, left.
    Border {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        widths: [i32; 4],
        radius: i32,
        color: Rgb,
    },
    /// A run of text on one line. `y` is the top of the font's natural line box
    /// (draw with the face's ascent below it), `h` its height.
    Text {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        text: String,
        color: Rgb,
        font: Font,
        /// [`DECO_UNDERLINE`] | [`DECO_STRIKE`].
        deco: u8,
        /// Index into [`Page::links`] when the run is link text.
        link: Option<u32>,
    },
    /// A decoded picture: paint `Page::images[idx]`'s pixels scaled into the box.
    Image {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        idx: usize,
    },
}
/// One `<img>` of the page: the `src` as written and the text to show when
/// there is no picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImgRef {
    pub src: String,
    /// `alt`, entities decoded.
    pub alt: String,
}
/// The box of one form control, in page coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldBox {
    pub form: usize,
    pub field: usize,
    pub kind: FieldKind,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Size in pixels of the control's text.
    pub size: u16,
    /// Inset of the text from the box's left edge.
    pub pad_x: i32,
    /// Height of one line of a `<textarea>` and the inset of its text from the top edge.
    pub line_h: i32,
    pub pad_y: i32,
}
/// A laid-out page: a flat display list plus the total content height (for
/// scrolling).
#[derive(Debug)]
pub struct Page {
    pub cmds: Vec<Cmd>,
    pub height: i32,
    /// The canvas colour (`html`/`body` background, white by default).
    pub background: Rgb,
    /// `href` of every `<a href>` on the page (at most [`MAX_LINKS`]).
    pub links: Vec<String>,
    /// Clickable boxes of link text, in page coordinates (same space as `cmds`).
    pub hits: Vec<LinkHit>,
    /// Every `<img>` (at most [`MAX_IMAGES`]); `Cmd::Image::idx` indexes this.
    pub images: Vec<ImgRef>,
    /// Every `<form>` with its controls (at most [`super::form::MAX_FORMS`]).
    pub forms: Vec<FormInfo>,
    /// Boxes of the visible controls, in document order.
    pub fields: Vec<FieldBox>,
}
impl Default for Page {
    fn default() -> Self {
        Page {
            cmds: Vec::new(),
            height: 0,
            background: Rgb(255, 255, 255),
            links: Vec::new(),
            hits: Vec::new(),
            images: Vec::new(),
            forms: Vec::new(),
            fields: Vec::new(),
        }
    }
}
/// Most links recorded per page (the rest still render, they are just not clickable).
pub const MAX_LINKS: usize = 2000;
/// Most `<img>` elements recorded per page (the rest are not laid out).
pub const MAX_IMAGES: usize = 200;
/// Tallest page, in pixels: every y coordinate saturates here, so a hostile page cannot
/// overflow the arithmetic of the layout, the scroll or the painter.
pub const MAX_PAGE_H: i32 = 1 << 24;
/// `y + d`, saturating at [`MAX_PAGE_H`].
fn adv(y: i32, d: i32) -> i32 {
    y.saturating_add(d).clamp(i32::MIN / 2, MAX_PAGE_H)
}
/// Most characters of text a page may put on screen (the rest is dropped).
pub const MAX_TEXT_CHARS: usize = 1_000_000;
/// Most display commands one layout may emit.
pub const MAX_CMDS: usize = 150_000;
/// Longest unbroken word kept, in characters (a longer one is cut).
pub const MAX_WORD_CHARS: usize = 4096;
/// Budget of ancestor steps the selector matching of one layout may take.
const STYLE_BUDGET: u32 = 4_000_000;
/// The box of one run of link text and the link it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkHit {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Index into [`Page::links`].
    pub link: usize,
}
impl Page {
    /// Index into [`Page::links`] of the link under page coordinates `(x, y)`, if any.
    pub fn link_index_at(&self, x: i32, y: i32) -> Option<usize> {
        self.hits
            .iter()
            .rev()
            .find(|h| x >= h.x && x < h.x + h.w && y >= h.y && y < h.y + h.h)
            .map(|h| h.link)
    }

    /// The `href` of the link under page coordinates `(x, y)`, if any.
    pub fn link_at(&self, x: i32, y: i32) -> Option<&str> {
        self.link_index_at(x, y)
            .and_then(|l| self.links.get(l))
            .map(String::as_str)
    }

    /// The control under page coordinates `(x, y)`, if any.
    pub fn field_at(&self, x: i32, y: i32) -> Option<&FieldBox> {
        self.fields
            .iter()
            .rev()
            .find(|f| x >= f.x && x < f.x + f.w && y >= f.y && y < f.y + f.h)
    }
}
/// How to lay a document out.
pub struct Layout<'a> {
    /// Viewport width in pixels.
    pub width: i32,
    /// Page zoom in percent ([`MIN_ZOOM`]..=[`MAX_ZOOM`], 100 = normal).
    pub zoom: u16,
    /// What is known about each `<img>`.
    pub images: &'a dyn ImageLookup,
    /// Where every text width comes from.
    pub metrics: &'a dyn TextMetrics,
}
/// Smallest and largest page zoom (percent).
pub const MIN_ZOOM: u16 = 50;
pub const MAX_ZOOM: u16 = 300;
/// The zoom steps Ctrl+plus / Ctrl+minus walk through.
pub const ZOOM_STEPS: [u16; 8] = [50, 75, 100, 125, 150, 200, 250, 300];
/// The next zoom step above `z` (or `z` itself at the top).
pub fn zoom_in(z: u16) -> u16 {
    ZOOM_STEPS
        .iter()
        .copied()
        .find(|&s| s > z)
        .unwrap_or(MAX_ZOOM)
}
/// The next zoom step below `z` (or `z` itself at the bottom).
pub fn zoom_out(z: u16) -> u16 {
    ZOOM_STEPS
        .iter()
        .rev()
        .copied()
        .find(|&s| s < z)
        .unwrap_or(MIN_ZOOM)
}
