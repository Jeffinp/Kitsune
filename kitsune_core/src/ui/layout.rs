//! Desktop geometry: taskbar, context menu, Apps launcher, calculator keypad,
//! browser chrome and file-manager hit zones.
//!
//! Pure integer layout shared by the kernel's drawing code and its hit-testing,
//! so the two can never disagree. Nothing here touches pixels.

use crate::windowing::window::{Rect, TITLE_H};
use alloc::vec::Vec;

/// Calculator keypad: the input byte for each cell, six rows of four. The first row is the
/// memory keys (`MC MR M- M+`, see `calc::KEY_*`), then `C`, backspace (`0x08`), percent and
/// divide; the digit rows each end in their operator; the last row is sign, `0`, the point
/// and equals.
pub const CALC_KEYS: [[u8; 4]; 6] = [
    [0x01, 0x02, 0x03, 0x04],
    [b'C', 0x08, b'%', b'/'],
    *b"789*",
    *b"456-",
    *b"123+",
    *b"n0.=",
];

/// Where everything sits in a calculator window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CalcGeom {
    /// The strip with the pending expression and the last results.
    pub history: Rect,
    /// The big number.
    pub display: Rect,
    /// The copy button, at the right end of the history strip.
    pub copy: Rect,
    /// Every key, row by row.
    pub keys: [[Rect; 4]; 6],
}

/// Height of the memory row; the other rows share the rest.
pub const CALC_MEM_H: i32 = 28;
/// Gap between keys.
pub const CALC_GAP: i32 = 8;

/// Geometry of calculator window `r` (the window rectangle, title bar included).
pub fn calc_geom(r: Rect) -> CalcGeom {
    let pad = 16;
    let x = r.x + pad;
    let w = (r.w - 2 * pad).max(0);
    let history = Rect::new(x, r.y + TITLE_H + 8, w, 36);
    let display = Rect::new(x, history.bottom(), w, 64);
    let copy = Rect::new(history.right() - 28, history.y + 2, 28, 24);
    let gy = display.bottom() + 8;
    let cw = ((w - CALC_GAP * 3) / 4).max(0);
    let grid_h = r.bottom() - pad - gy;
    let ch = ((grid_h - CALC_MEM_H - CALC_GAP * 5) / 5).max(0);
    let mut keys = [[Rect::new(0, 0, 0, 0); 4]; 6];
    let mut y = gy;
    for (row, cells) in keys.iter_mut().enumerate() {
        let h = if row == 0 { CALC_MEM_H } else { ch };
        for (col, cell) in cells.iter_mut().enumerate() {
            *cell = Rect::new(x + col as i32 * (cw + CALC_GAP), y, cw, h);
        }
        y += h + CALC_GAP;
    }
    CalcGeom {
        history,
        display,
        copy,
        keys,
    }
}

/// What a click in a calculator window hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalcHit {
    /// A key, by its input byte.
    Key(u8),
    /// The copy button, or the display itself.
    Copy,
}

/// The key (or the copy action) under `(px, py)` in calculator window `r`, if any.
pub fn calc_hit(r: Rect, px: i32, py: i32) -> Option<CalcHit> {
    let g = calc_geom(r);
    if g.copy.contains(px, py) || g.display.contains(px, py) {
        return Some(CalcHit::Copy);
    }
    for (row, cells) in g.keys.iter().enumerate() {
        for (col, cell) in cells.iter().enumerate() {
            if cell.w > 0 && cell.h > 0 && cell.contains(px, py) {
                return Some(CalcHit::Key(CALC_KEYS[row][col]));
            }
        }
    }
    None
}

/// The keypad byte under `(px, py)` in calculator window `r`, if any.
pub fn calc_button_at(r: Rect, px: i32, py: i32) -> Option<u8> {
    match calc_hit(r, px, py)? {
        CalcHit::Key(k) => Some(k),
        CalcHit::Copy => None,
    }
}

// --------------------------------------------------------------------- browser

/// Height of the toolbar band under the title bar.
pub const BROWSER_TOOLBAR_H: i32 = 48;
/// Height of the tab strip (shown from two tabs on).
pub const BROWSER_STRIP_H: i32 = 36;
/// Side of the toolbar buttons.
pub const BROWSER_BTN: i32 = 32;
/// Height of a tab.
pub const BROWSER_TAB_H: i32 = 28;
/// Widest and narrowest a tab gets.
pub const BROWSER_TAB_MAX_W: i32 = 208;
pub const BROWSER_TAB_MIN_W: i32 = 72;

/// Browser chrome geometry, shared by drawing and hit-testing so the toolbar
/// buttons, omnibox, tabs and content area always agree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BrowserChrome {
    /// The band under the title bar that holds the buttons and the omnibox.
    pub toolbar: Rect,
    pub back: Rect,
    pub forward: Rect,
    pub reload: Rect,
    /// The omnibox.
    pub bar: Rect,
    /// The security indicator, inside the left end of the omnibox (empty when there is none).
    pub shield: Rect,
    /// The favourite star, inside the right end of the omnibox.
    pub star: Rect,
    /// The ad blocker's shield and its count, inside the omnibox just left of the star (empty when
    /// the page has none, see [`BrowserChrome::with_blocker`]).
    pub blocker: Rect,
    /// The "new tab" button right of the omnibox.
    pub newtab: Rect,
    /// The loading progress track under the omnibox.
    pub progress: Rect,
    /// The tab strip (zero height with a single tab).
    pub strip: Rect,
    /// The page area, edge to edge below the chrome.
    pub content: Rect,
}

impl BrowserChrome {
    /// Geometry for window `r` with `tabs` tabs; `shield_w` is the width of the security
    /// indicator (0 for none: the start page and internal pages).
    pub fn of(r: Rect, tabs: usize, shield_w: i32) -> Self {
        Self::with_strip(r, shield_w, if tabs >= 2 { BROWSER_STRIP_H } else { 0 })
    }

    /// [`BrowserChrome::of`] with the strip `strip_h` pixels tall (it grows and shrinks while a
    /// second tab opens or the second to last closes).
    pub fn with_strip(r: Rect, shield_w: i32, strip_h: i32) -> Self {
        let pad = 12;
        let y0 = r.y + TITLE_H;
        let toolbar = Rect::new(r.x, y0, r.w, BROWSER_TOOLBAR_H);
        let by = y0 + (BROWSER_TOOLBAR_H - BROWSER_BTN) / 2;
        let back = Rect::new(r.x + pad, by, BROWSER_BTN, BROWSER_BTN);
        let forward = Rect::new(back.right() + 4, by, BROWSER_BTN, BROWSER_BTN);
        let reload = Rect::new(forward.right() + 8, by, BROWSER_BTN, BROWSER_BTN);
        let newtab = Rect::new(r.right() - pad - BROWSER_BTN, by, BROWSER_BTN, BROWSER_BTN);
        let bar_x = reload.right() + 12;
        let bar = Rect::new(bar_x, by, (newtab.x - 8 - bar_x).max(120), BROWSER_BTN);
        let shield = if shield_w > 0 {
            Rect::new(bar.x + 4, bar.y + 4, shield_w.min(bar.w / 2), 24)
        } else {
            Rect::new(bar.x + 4, bar.y + 4, 0, 24)
        };
        let star = Rect::new(bar.right() - 4 - 24, bar.y + 4, 24, 24);
        let progress = Rect::new(bar.x + 12, bar.bottom() + 3, (bar.w - 24).max(0), 2);
        let strip = Rect::new(
            r.x,
            toolbar.bottom(),
            r.w,
            strip_h.clamp(0, BROWSER_STRIP_H),
        );
        let cy = strip.bottom();
        let content = Rect::new(r.x, cy, r.w, (r.bottom() - cy).max(0));
        Self {
            toolbar,
            back,
            forward,
            reload,
            bar,
            shield,
            star,
            blocker: Rect::new(star.x - 4, bar.y + 4, 0, 24),
            newtab,
            progress,
            strip,
            content,
        }
    }

    /// The same chrome with the ad blocker's button `w` pixels wide (0: none) placed left of the star.
    pub fn with_blocker(mut self, w: i32) -> Self {
        let w = w.clamp(0, (self.bar.w / 3).max(0));
        self.blocker = Rect::new(self.star.x - 4 - w, self.bar.y + 4, w, 24);
        self
    }

    /// Right edge of the address text inside the omnibox: before the blocker's button and the star.
    pub fn text_right(&self, has_star: bool) -> i32 {
        let edge = if self.blocker.w > 0 {
            self.blocker.x - 4
        } else if has_star {
            self.star.x - 4
        } else {
            self.bar.right() - 12
        };
        edge.max(self.text_x() + 8)
    }

    /// Left edge of the address text inside the omnibox.
    pub fn text_x(&self) -> i32 {
        if self.shield.w > 0 {
            self.shield.right() + 8
        } else {
            self.bar.x + 36
        }
    }
}

/// The tabs of a strip: one rectangle per entry of `weights` (0..=256, how far each tab
/// has grown while it opens or closes), left to right with 4 px between them. Tabs
/// share the strip equally up to [`BROWSER_TAB_MAX_W`] and never get below
/// [`BROWSER_TAB_MIN_W`] (the strip then overflows and the caller scrolls or clips).
pub fn browser_tab_rects(strip: Rect, weights: &[i32]) -> Vec<Rect> {
    let n = weights.len() as i32;
    if n == 0 {
        return Vec::new();
    }
    let total: i32 = weights.iter().map(|w| (*w).clamp(0, 256)).sum();
    let avail = (strip.w - 24 - 4 * (n - 1)).max(0);
    // One full tab's width: the room shared by the full tabs (a growing one counts less).
    let slot = if total == 0 {
        BROWSER_TAB_MAX_W
    } else {
        (avail * 256 / total).clamp(BROWSER_TAB_MIN_W, BROWSER_TAB_MAX_W)
    };
    let mut x = strip.x + 12;
    let y = strip.y + (strip.h - BROWSER_TAB_H) / 2;
    weights
        .iter()
        .map(|w| {
            let w = (*w).clamp(0, 256);
            let tw = slot * w / 256;
            let r = Rect::new(x, y, tw, BROWSER_TAB_H);
            x += tw + if w > 0 { 4 * w / 256 } else { 0 };
            r
        })
        .collect()
}

/// The close button inside tab `tab`.
pub fn browser_tab_close(tab: Rect) -> Rect {
    Rect::new(tab.right() - 6 - 20, tab.y + (tab.h - 20) / 2, 20, 20)
}

/// The index of the tab under `(x, y)`.
pub fn browser_tab_at(rects: &[Rect], x: i32, y: i32) -> Option<usize> {
    rects.iter().position(|r| r.w > 0 && r.contains(x, y))
}

/// Rows of the suggestion list are 32 px tall.
pub const BROWSER_SUGGEST_ROW: i32 = 32;

/// The panel of `n` suggestions hanging under the omnibox `bar`.
pub fn browser_suggest_panel(bar: Rect, n: usize) -> Rect {
    Rect::new(
        bar.x,
        bar.bottom() + 6,
        bar.w,
        n as i32 * BROWSER_SUGGEST_ROW + 12,
    )
}

/// Rect of suggestion row `i` under the omnibox.
pub fn browser_suggestion_row(bar: Rect, i: usize) -> Rect {
    let p = browser_suggest_panel(bar, 0);
    Rect::new(
        p.x + 6,
        p.y + 6 + i as i32 * BROWSER_SUGGEST_ROW,
        bar.w - 12,
        BROWSER_SUGGEST_ROW,
    )
}

/// Row of the suggestion list under point `(px, py)` when `n` rows are shown.
pub fn browser_suggestion_at(bar: Rect, n: usize, px: i32, py: i32) -> Option<usize> {
    (0..n).find(|&i| browser_suggestion_row(bar, i).contains(px, py))
}

/// The popover that explains the security indicator, `h` tall, under the omnibox's left end,
/// kept inside window `win`.
pub fn browser_popover(bar: Rect, win: Rect, h: i32) -> Rect {
    let w = 328.min(win.w - 16).max(120);
    let x = bar.x.clamp(win.x + 8, (win.right() - w - 8).max(win.x + 8));
    Rect::new(x, bar.bottom() + 8, w, h)
}

/// The popover under the ad blocker's button `blocker`, `h` tall, kept inside window `win`: its right
/// edge lines up with the button's.
pub fn browser_blocker_popover(blocker: Rect, bar: Rect, win: Rect, h: i32) -> Rect {
    let w = 312.min(win.w - 16).max(120);
    let x = (blocker.right() - w).clamp(win.x + 8, (win.right() - w - 8).max(win.x + 8));
    Rect::new(x, bar.bottom() + 8, w, h)
}

/// Where everything of the find bar goes: a slim bar in the top right of the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FindLayout {
    pub bar: Rect,
    pub field: Rect,
    pub count: Rect,
    pub prev: Rect,
    pub next: Rect,
    pub close: Rect,
}

/// Layout of the find bar in page area `content`.
pub fn browser_find_layout(content: Rect) -> FindLayout {
    let w = 372.min(content.w - 24).max(200);
    let bar = Rect::new(
        (content.right() - 12 - w).max(content.x + 4),
        content.y + 12,
        w,
        40,
    );
    let close = Rect::new(bar.right() - 8 - 24, bar.y + 8, 24, 24);
    let next = Rect::new(close.x - 4 - 24, bar.y + 8, 24, 24);
    let prev = Rect::new(next.x - 24, bar.y + 8, 24, 24);
    let count = Rect::new(prev.x - 8 - 64, bar.y + 8, 64, 24);
    let field = Rect::new(
        bar.x + 8,
        bar.y + 6,
        (count.x - 8 - (bar.x + 8)).max(40),
        28,
    );
    FindLayout {
        bar,
        field,
        count,
        prev,
        next,
        close,
    }
}

/// The zoom pill at the bottom right of the page.
pub fn browser_zoom_pill(content: Rect) -> Rect {
    Rect::new(
        content.right() - 16 - 72,
        content.bottom() - 16 - 28,
        72,
        28,
    )
}

/// Where the parts of an error page go, centred in the page area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErrorLayout {
    /// The illustration.
    pub art: Rect,
    pub title: Rect,
    pub cause: Rect,
    pub retry: Rect,
    /// "Continue anyway" (certificate errors only).
    pub proceed: Rect,
    /// The one-line warning under it.
    pub note: Rect,
}

/// Layout of an error page in `content`; `cert` adds the "continue anyway" row.
pub fn browser_error_layout(content: Rect, cert: bool) -> ErrorLayout {
    let cw = (content.w - 48).clamp(120, 520);
    let cx = content.x + content.w / 2;
    // The block is centred vertically a little above the middle.
    let block_h = 96 + 24 + 32 + 8 + 24 + 28 + 40 + if cert { 12 + 32 + 8 + 20 } else { 0 };
    let top = content.y + ((content.h - block_h) / 2 - 12).max(16);
    let art = Rect::new(cx - 48, top, 96, 96);
    let title = Rect::new(cx - cw / 2, art.bottom() + 24, cw, 32);
    let cause = Rect::new(cx - cw / 2, title.bottom() + 8, cw, 24);
    let retry = Rect::new(cx - 88, cause.bottom() + 28, 176, 40);
    let pw = (cw).min(300);
    let proceed = Rect::new(cx - pw / 2, retry.bottom() + 12, pw, 32);
    let note = Rect::new(cx - cw / 2, proceed.bottom() + 8, cw, 20);
    ErrorLayout {
        art,
        title,
        cause,
        retry,
        proceed,
        note,
    }
}

/// Where the parts of the new-tab page go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartLayout {
    /// The big search field.
    pub search: Rect,
    /// Heading rows of the tile grid and of the recent list.
    pub tiles_heading: Rect,
    pub tiles: Vec<Rect>,
    pub recent_heading: Rect,
    pub recents: Vec<Rect>,
}

/// Layout of the new-tab page in `content` for up to `nt` tiles and `nr` recent rows; what
/// does not fit is left out.
pub fn browser_start_layout(content: Rect, nt: usize, nr: usize) -> StartLayout {
    let cw = (content.w - 48).clamp(160, 600);
    let cx = content.x + content.w / 2;
    let left = cx - cw / 2;
    let search = Rect::new(left, content.y + (content.h / 8).clamp(24, 72), cw, 48);
    let tiles_heading = Rect::new(left, search.bottom() + 28, cw, 20);
    // Tiles: as many columns of 104 px as fit (at most 5), 16 px apart.
    let cols = (((cw + 16) / (104 + 16)).clamp(1, 5)) as usize;
    let tile_w = (cw - 16 * (cols as i32 - 1)) / cols as i32;
    let tile_h = 96;
    let mut tiles = Vec::new();
    let mut y = tiles_heading.bottom() + 12;
    let room_for_rows = |y: i32, extra: i32| y + tile_h + extra <= content.bottom() - 8;
    let mut i = 0;
    while i < nt && room_for_rows(y, 0) {
        let row = (nt - i).min(cols);
        for k in 0..row {
            tiles.push(Rect::new(
                left + k as i32 * (tile_w + 16),
                y,
                tile_w,
                tile_h,
            ));
        }
        i += row;
        y += tile_h + 16;
    }
    let tiles_bottom = if tiles.is_empty() {
        tiles_heading.bottom()
    } else {
        y - 16
    };
    let recent_heading = Rect::new(left, tiles_bottom + 24, cw, 20);
    let mut recents = Vec::new();
    let mut ry = recent_heading.bottom() + 8;
    for _ in 0..nr {
        if ry + 40 > content.bottom() - 8 {
            break;
        }
        recents.push(Rect::new(left, ry, cw, 40));
        ry += 40;
    }
    StartLayout {
        search,
        tiles_heading,
        tiles,
        recent_heading,
        recents,
    }
}

// ------------------------------------------------------------- work area & fit

/// Gap kept between the work area and the floating taskbar.
pub const WORK_DOCK_GAP: i32 = 8;

/// The rectangle windows maximize and snap into: the screen below the top panel, edge to edge,
/// down to the taskbar (plus a small gap).
pub fn work_area(sw: i32, sh: i32) -> Rect {
    let dock_top = sh - crate::windowing::taskbar::BOTTOM - crate::windowing::taskbar::H;
    let top = crate::windowing::window::MENUBAR_H;
    Rect::new(0, top, sw.max(0), (dock_top - WORK_DOCK_GAP - top).max(0))
}

/// Largest integer text scale in `base..=max` at which a text grid fits an
/// `avail_w x avail_h` area. The grid measures `grid_w x grid_h` pixels at
/// scale 1. When even `base` does not fit, `base` is returned (the caller
/// clips). Apps with a fixed logical grid (terminal, editor) use this so a
/// maximized window shows bigger, undistorted text instead of empty space.
pub fn fit_scale(avail_w: i32, avail_h: i32, grid_w: i32, grid_h: i32, base: i32, max: i32) -> i32 {
    let mut best = base;
    let mut s = base + 1;
    while s <= max {
        if grid_w * s <= avail_w && grid_h * s <= avail_h {
            best = s;
        }
        s += 1;
    }
    best
}

// ---------------------------------------------------------------- file manager

/// Width of the file manager's sidebar.
pub const FILES_SIDEBAR_W: i32 = 176;
/// Height of one file-list row.
pub const FILES_ROW_H: i32 = 30;
/// Offset of the first list row below the title bar.
const FILES_LIST_TOP: i32 = 78;

/// What a click inside the file manager landed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilesHit {
    /// A sidebar entry: switch to this view.
    View(u8),
    /// A row of the main list (0-based, not yet bounds-checked against the
    /// number of entries).
    Row(usize),
}

/// Resolves a click at `(px, py)` in a file-manager window `rect` currently
/// showing `view` (0 = files, 1 = trash; rows only exist in those views).
pub fn files_hit(rect: Rect, view: u8, px: i32, py: i32) -> Option<FilesHit> {
    let cy0 = rect.y + TITLE_H;
    if px < rect.x + FILES_SIDEBAR_W {
        let zones = [
            (34, 64, 0u8),
            (66, 96, 1),
            (132, 162, 2),
            (164, 194, 3),
            (208, 238, 4),
        ];
        return zones
            .iter()
            .find(|&&(a, b, _)| (cy0 + a..cy0 + b).contains(&py))
            .map(|&(_, _, v)| FilesHit::View(v));
    }
    if view <= 1 {
        let list_y = cy0 + FILES_LIST_TOP;
        if py >= list_y {
            return Some(FilesHit::Row(((py - list_y) / FILES_ROW_H) as usize));
        }
    }
    None
}

#[cfg(test)]
mod tests;
