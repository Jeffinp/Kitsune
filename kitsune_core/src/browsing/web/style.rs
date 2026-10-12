//! Style cascade: combine the UA + page stylesheets into computed values.
//!
//! Lengths stay in CSS pixels here (`em` and `rem` are resolved against the
//! element's own font size); the page zoom is applied by the layout.

use super::css::{Decl, Specificity, Stylesheet, parse_decls};
use super::dom::Element;
use super::{Rgb, parse_color, parse_color_over};
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

mod calc;
mod cascade;
mod values;
pub(crate) use cascade::*;
pub(crate) use values::*;

// ---- the user-agent sheet ----

/// The baseline look pages get when they say nothing: system-like text, block
/// structure, readable headings, indigo links. Written in the same CSS the engine
/// reads from pages, so one parser covers both.
pub(crate) const UA_CSS: &str = "
html,body,div,p,h1,h2,h3,h4,h5,h6,ul,ol,dl,dt,dd,header,footer,article,section,nav,main,aside,blockquote,pre,form,figure,figcaption,address,details,summary,fieldset,legend,hgroup,center,hr,menu,dir,caption{display:block}
li{display:list-item}
table{display:table}
tr{display:table-row}
td,th{display:table-cell}
thead,tbody,tfoot{display:table-row-group}
script,style,head,title,meta,link,option,datalist,template,noembed,svg,canvas,video,audio,iframe,object,embed,map,area,param,source,track,base,dialog,col,colgroup{display:none}
html{color:#1d1d1f;font-size:16px;line-height:1.5}
body{margin:8px}
h1,h2,h3,h4,h5,h6{font-weight:bold;line-height:1.25}
h1{font-size:2em;margin:.67em 0}
h2{font-size:1.5em;margin:.83em 0}
h3{font-size:1.17em;margin:1em 0}
h4{font-size:1em;margin:1.33em 0}
h5{font-size:.83em;margin:1.67em 0}
h6{font-size:.67em;margin:2.33em 0}
p{margin:1em 0}
blockquote{margin:1em 0;padding:.25em 1em;border-left:3px solid #c7c7cc;color:#515154}
ul,ol,menu,dir{margin:1em 0;padding-left:28px}
ul{list-style-type:disc}
ol{list-style-type:decimal}
ul ul,ul ol,ol ul,ol ol{margin:0}
ul ul{list-style-type:circle}
ul ul ul{list-style-type:square}
dl{margin:1em 0}
dt{font-weight:bold}
dd{margin-left:32px}
a{color:#4f46e5}
b,strong{font-weight:bold}
i,em,cite,dfn,var,address{font-style:italic}
u,ins{text-decoration:underline}
s,strike,del{text-decoration:line-through}
small{font-size:.83em}
big{font-size:1.17em}
sub,sup{font-size:.75em}
sub{vertical-align:sub}
sup{vertical-align:super}
mark{background:#fff3a3;color:#1d1d1f}
code,kbd,samp,tt{font-family:monospace;font-size:.92em}
code,kbd{background:#f1f1f5;border-radius:4px}
pre{font-family:monospace;font-size:.9em;white-space:pre;margin:1em 0;padding:12px 14px;background:#f5f5f7;border-radius:8px;line-height:1.4}
pre code{background:none;font-size:1em}
hr{margin:1.2em 0;border-top:1px solid #d1d1d6;height:0}
center{text-align:center}
caption{text-align:center;font-weight:bold;padding:4px 0}
th{font-weight:bold;text-align:center}
td,th{padding:4px 8px}
figure{margin:1em 40px}
figcaption{font-size:.9em;color:#6e6e73}
fieldset{margin:1em 0;padding:.5em 1em;border:1px solid #d1d1d6;border-radius:8px}
legend{font-weight:bold}
summary{font-weight:bold}
";

// ---- computed values ----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Disp {
    Block,
    Inline,
    /// `display: flex` / `inline-flex`: a block whose children are laid out by `layout/flex.rs`.
    Flex,
    /// `display: grid` / `inline-grid`.
    Grid,
    /// `display: inline-block`: shrink-to-fit box that sits in a line (a run of them is laid out as a row).
    InlineBlock,
    ListItem,
    None,
    Table,
    TableRowGroup,
    TableRow,
    TableCell,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Float {
    None,
    Left,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Clear {
    None,
    Left,
    Right,
    Both,
}

/// `justify-content`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Justify {
    Start,
    Center,
    End,
    Between,
    Around,
    Evenly,
}

/// `align-items` / `align-self` (`None` for `auto`/`normal`: stretch, or the container's value).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CrossAlign {
    Stretch,
    Start,
    Center,
    End,
}

/// The flexbox and grid properties of an element (as container and as item).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FlexStyle {
    /// `flex-direction: row` (false: column).
    pub(crate) row: bool,
    pub(crate) reverse: bool,
    pub(crate) wrap: bool,
    pub(crate) justify: Justify,
    pub(crate) align: CrossAlign,
    /// `align-self`, when not `auto`.
    pub(crate) align_self: Option<CrossAlign>,
    /// Row gap and column gap, in CSS px.
    pub(crate) gap_row: i32,
    pub(crate) gap_col: i32,
    /// `flex-grow`, `flex-shrink` in thousandths.
    pub(crate) grow: i32,
    pub(crate) shrink: i32,
    pub(crate) basis: Len,
    pub(crate) order: i32,
    /// Grid: `grid-template-columns`, and how many columns an item spans.
    pub(crate) tracks: Rc<GridSpec>,
    pub(crate) span: u8,
}

impl Default for FlexStyle {
    fn default() -> Self {
        FlexStyle {
            row: true,
            reverse: false,
            wrap: false,
            justify: Justify::Start,
            align: CrossAlign::Stretch,
            align_self: None,
            gap_row: 0,
            gap_col: 0,
            grow: 0,
            shrink: 1000,
            basis: Len::Auto,
            order: 0,
            tracks: Rc::new(GridSpec::default()),
            span: 1,
        }
    }
}

/// One column of a grid.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Track {
    Px(i32),
    Pct(i32),
    /// `fr` in thousandths.
    Fr(i32),
    Auto,
}

/// `grid-template-columns`: explicit columns, plus an `auto-fill`/`auto-fit` repeat
/// (`repeat(auto-fill, minmax(min, 1fr))`: as many as fit, at least `min` px wide each).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GridSpec {
    pub(crate) cols: Vec<Track>,
    pub(crate) auto_min: Option<i32>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Pos {
    Static,
    /// `absolute` and `fixed`: taken out of the flow by browsers; here shown in the flow unless
    /// they are clearly meant to be invisible (see `cascade::hidden_by_position`).
    Abs,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Align {
    Left,
    Center,
    Right,
}

/// A length that may depend on the container.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Len {
    /// CSS pixels.
    Px(i32),
    /// Percent of the container's width.
    Pct(i32),
    /// `calc()` and friends: `pct` thousandths of a percent of the container plus `px`; `mode` 0 is the
    /// sum, 1 the smaller and 2 the larger of the two parts (`min(100%, 600px)`).
    Calc {
        pct: i32,
        px: i32,
        mode: u8,
    },
    Auto,
}

impl Len {
    /// Resolve against a container `base` pixels wide; `zoom` percent applies to `Px`.
    pub(crate) fn resolve(self, base: i32, zoom: i32) -> Option<i32> {
        match self {
            Len::Px(v) => Some(zoom_px(v, zoom)),
            Len::Pct(p) => {
                Some((i64::from(base) * i64::from(p) / 100).clamp(-100_000, 100_000) as i32)
            }
            Len::Calc { pct, px, mode } => {
                let a =
                    (i64::from(base) * i64::from(pct) / 100_000).clamp(-100_000, 100_000) as i32;
                let b = zoom_px(px, zoom);
                Some(match mode {
                    1 => a.min(b),
                    2 => a.max(b),
                    _ => a.saturating_add(b),
                })
            }
            Len::Auto => None,
        }
    }
}

/// Scale a CSS pixel length by the zoom percent.
pub(crate) fn zoom_px(v: i32, zoom: i32) -> i32 {
    (i64::from(v) * i64::from(zoom) / 100).clamp(-100_000, 100_000) as i32
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LineH {
    Normal,
    /// Percent of the font size (150 = 1.5).
    Mult(i32),
    Px(i32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ListStyle {
    None,
    Disc,
    Circle,
    Square,
    Decimal,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
}

/// Where a table cell's content sits in its row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum VAlign {
    Top,
    Middle,
    Bottom,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Ws {
    Normal,
    /// Whitespace and newlines kept, long lines still wrap (we cannot scroll sideways).
    Pre,
    NoWrap,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Transform {
    None,
    Upper,
    Lower,
    Capital,
}

/// Computed style for one element: only the properties our renderer uses.
#[derive(Clone, Debug)]
pub(crate) struct Computed {
    pub(crate) display: Disp,
    pub(crate) color: Rgb,
    pub(crate) bg: Option<Rgb>,
    /// Font size in CSS pixels.
    pub(crate) font_px: i32,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) mono: bool,
    pub(crate) underline: bool,
    pub(crate) strike: bool,
    pub(crate) line_h: LineH,
    pub(crate) align: Align,
    pub(crate) ws: Ws,
    pub(crate) transform: Transform,
    pub(crate) list: ListStyle,
    /// top, right, bottom, left.
    pub(crate) margin: [Len; 4],
    pub(crate) padding: [Len; 4],
    pub(crate) border_w: [i32; 4],
    pub(crate) border_color: Rgb,
    pub(crate) radius: i32,
    pub(crate) width: Len,
    pub(crate) max_width: Len,
    /// `height`, used as a minimum height (CSS pixels, 0 = none).
    pub(crate) min_height: i32,
    /// `box-sizing: border-box`.
    pub(crate) border_box: bool,
    /// Table: `border-spacing` (CSS px) and `border-collapse: collapse`.
    pub(crate) spacing: i32,
    pub(crate) collapse: bool,
    /// Table cell: vertical alignment.
    pub(crate) valign: VAlign,
    /// Inline: raised (1, `sup`) or lowered (-1, `sub`) text.
    pub(crate) vshift: i8,
    /// Custom properties (`--name`), inherited; shared until an element sets one.
    pub(crate) vars: Rc<Vars>,
    /// What is behind this element's content: the nearest ancestor's background (white at the top).
    /// Translucent colours are blended over it.
    pub(crate) backdrop: Rgb,
    pub(crate) float: Float,
    pub(crate) clear: Clear,
    /// The element makes its own block formatting context (`overflow` other than visible, ...): it
    /// holds its floats.
    pub(crate) bfc: bool,
    /// A `::after { clear: both }` rule matched: the element holds its floats.
    pub(crate) clearfix: bool,
    pub(crate) position: Pos,
    /// `min-width` (px or %).
    pub(crate) min_width: Len,
    pub(crate) flex: FlexStyle,
    /// Hidden on purpose (`visibility: hidden`, `opacity: 0`, clipped to nothing, shifted off the
    /// page): laid out as `display: none`.
    pub(crate) hidden: bool,
    /// `text-indent` in px (a large negative one hides the text of an image-replacement box).
    pub(crate) indent: i32,
    /// `clip` / `clip-path` given (with `position: absolute` and a tiny box: a visually-hidden label).
    pub(crate) clipped: bool,
    /// Offsets `left`, `top` far off the page.
    pub(crate) offscreen: bool,
    /// `content` of a `::before` / `::after` box (decoded; `None` = no box).
    pub(crate) content: Option<String>,
}

/// The custom properties in effect: name (lower case, with the `--`) to its text.
pub(crate) type Vars = BTreeMap<String, String>;

impl Computed {
    /// The inherited root style (before any element).
    pub(crate) fn root() -> Self {
        Computed {
            display: Disp::Block,
            color: Rgb(0x1d, 0x1d, 0x1f),
            bg: None,
            font_px: 16,
            bold: false,
            italic: false,
            mono: false,
            underline: false,
            strike: false,
            line_h: LineH::Mult(150),
            align: Align::Left,
            ws: Ws::Normal,
            transform: Transform::None,
            list: ListStyle::Disc,
            margin: [Len::Px(0); 4],
            padding: [Len::Px(0); 4],
            border_w: [0; 4],
            border_color: Rgb(0x1d, 0x1d, 0x1f),
            radius: 0,
            width: Len::Auto,
            max_width: Len::Auto,
            min_height: 0,
            border_box: false,
            spacing: 2,
            collapse: false,
            valign: VAlign::Middle,
            vshift: 0,
            vars: Rc::new(Vars::new()),
            backdrop: Rgb(255, 255, 255),
            float: Float::None,
            clear: Clear::None,
            bfc: false,
            clearfix: false,
            position: Pos::Static,
            min_width: Len::Auto,
            flex: FlexStyle::default(),
            hidden: false,
            indent: 0,
            clipped: false,
            offscreen: false,
            content: None,
        }
    }
}

fn default_display(tag: &str) -> Disp {
    match tag {
        "script" | "style" | "head" | "title" | "meta" | "link" | "option" | "datalist" => {
            Disp::None
        }
        _ => Disp::Inline,
    }
}

/// Upper bound for any CSS length we honour (px). Far beyond any real layout
/// value, and small enough that summing it over a whole page stays in `i32`.
pub(crate) const MAX_PX: i32 = 4096;

/// Smallest and largest font size honoured (CSS px).
const MIN_FONT: i32 = 4;
const MAX_FONT: i32 = 300;

#[cfg(test)]
mod tests;
