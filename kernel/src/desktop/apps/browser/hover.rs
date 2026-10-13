//! What the pointer is over in the browser: links, the start page and the page menu.

use crate::desktop::*;
use crate::text;
use kitsune_core::browser::tabs;

impl Desktop {
    /// The pointer moved: what is under it? Returns whether that changed (the window repaints
    /// the hover looks and the underline of a link).
    pub(crate) fn browser_hover_update(&mut self, cx: i32, cy: i32) -> bool {
        let Some(id) = self.browser_id() else {
            return false;
        };
        let over =
            self.drag.is_none() && !self.overlay_open() && self.topmost_at(cx, cy) == Some(id);
        let Some(rect) = self.browser_win_rect(id) else {
            return false;
        };
        let Some(b) = self.browser_state_mut(id) else {
            return false;
        };
        let target = if over {
            browser_hover_at(b, rect, cx, cy)
        } else {
            (BrowserHover::None, None)
        };
        let changed = target.0 != b.hover || target.1 != b.tabs.active().hover_link;
        b.hover = target.0;
        b.tabs.active_mut().hover_link = target.1;
        // The hovered link is part of the page cache key, so a new underline repaints by itself.
        changed
    }
}

/// What is under `(cx, cy)` in browser window `r`, and which link of the page.
pub(crate) fn browser_hover_at(
    b: &BrowserState,
    r: Rect,
    cx: i32,
    cy: i32,
) -> (BrowserHover, Option<usize>) {
    let ch = b.chrome(r);
    let t = b.tabs.active();
    if let Some(m) = &b.ctx {
        if let Some(i) = page_menu_row_at(m, r, cx, cy) {
            return (BrowserHover::MenuRow(i), None);
        }
        return (BrowserHover::None, None);
    }
    if b.shield_pop {
        let g = super::shield::popover_geom(b, &ch, r);
        if g.row.contains(cx, cy) && b.blocker_mode() != super::shield::BlockerMode::GlobalOff {
            return (BrowserHover::BlockerSwitch, None);
        }
        if g.pop.contains(cx, cy) {
            return (BrowserHover::None, None);
        }
    }
    let n = t.browser.suggestions().len();
    if let Some(i) =
        kitsune_core::layout::browser_suggestion_at(ch.bar, n, cx, cy).filter(|_| n > 0)
    {
        return (BrowserHover::Suggestion(i), None);
    }
    if ch.back.contains(cx, cy) {
        return (BrowserHover::Back, None);
    }
    if ch.forward.contains(cx, cy) {
        return (BrowserHover::Forward, None);
    }
    if ch.reload.contains(cx, cy) {
        return (BrowserHover::Reload, None);
    }
    if ch.newtab.contains(cx, cy) {
        return (BrowserHover::NewTab, None);
    }
    if ch.star.contains(cx, cy) {
        return (BrowserHover::Star, None);
    }
    if ch.blocker.w > 0 && ch.blocker.contains(cx, cy) {
        return (BrowserHover::Blocker, None);
    }
    if ch.shield.w > 0 && ch.shield.contains(cx, cy) {
        return (BrowserHover::Shield, None);
    }
    if ch.bar.contains(cx, cy) {
        return (BrowserHover::Bar, None);
    }
    if ch.strip.h > 0 && ch.strip.contains(cx, cy) {
        let rects = kitsune_core::layout::browser_tab_rects(ch.strip, &b.strip_weights());
        let hit = kitsune_core::layout::browser_tab_at(&rects, cx, cy).and_then(|ei| {
            let slots = b.strip_tab_slots();
            slots
                .iter()
                .find(|(e, _)| *e == ei)
                .map(|&(_, ti)| (ei, ti))
        });
        if let Some((ei, ti)) = hit {
            if kitsune_core::layout::browser_tab_close(rects[ei]).contains(cx, cy) {
                return (BrowserHover::TabClose(ti), None);
            }
            return (BrowserHover::Tab(ti), None);
        }
        return (BrowserHover::None, None);
    }
    if ch.content.contains(cx, cy) {
        if t.find.is_open() {
            let f = kitsune_core::layout::browser_find_layout(ch.content);
            if f.prev.contains(cx, cy) {
                return (BrowserHover::FindPrev, None);
            }
            if f.next.contains(cx, cy) {
                return (BrowserHover::FindNext, None);
            }
            if f.close.contains(cx, cy) {
                return (BrowserHover::FindClose, None);
            }
            if f.bar.contains(cx, cy) {
                return (BrowserHover::None, None);
            }
        }
        if t.browser.is_home() {
            return (start_hover_at(b, ch.content, cx, cy), None);
        }
        if t.page.is_none() && t.browser.status() == kitsune_core::browser::Status::Error {
            let e = kitsune_core::layout::browser_error_layout(
                ch.content,
                t.browser.can_continue_insecure(),
            );
            if e.retry.contains(cx, cy) {
                return (BrowserHover::Retry, None);
            }
            if t.browser.can_continue_insecure() && e.proceed.contains(cx, cy) {
                return (BrowserHover::Proceed, None);
            }
            return (BrowserHover::None, None);
        }
        if let Some(p) = &t.page {
            let (qx, qy) = (cx - ch.content.x, cy - ch.content.y + t.scroll);
            return (BrowserHover::None, p.link_index_at(qx, qy));
        }
    }
    (BrowserHover::None, None)
}

/// The tile or row of the start page under the pointer.
fn start_hover_at(b: &BrowserState, content: Rect, cx: i32, cy: i32) -> BrowserHover {
    let (tiles, recents) = start_items(b);
    let l = kitsune_core::layout::browser_start_layout(content, tiles.len(), recents.len());
    if let Some(i) = l.tiles.iter().position(|r| r.contains(cx, cy)) {
        return BrowserHover::Tile(i);
    }
    if let Some(i) = l.recents.iter().position(|r| r.contains(cx, cy)) {
        return BrowserHover::Recent(i);
    }
    BrowserHover::None
}

/// What the new-tab page lists: tiles `(label, url)` and recent addresses.
pub(crate) fn start_items(b: &BrowserState) -> (Vec<(String, String)>, Vec<String>) {
    let t = b.tabs.active();
    let bm = t.browser.bookmarks();
    let mut tiles: Vec<(String, String)> = bm
        .iter()
        .take(10)
        .map(|k| (tabs::tab_title(&k.title, &k.url), k.url.clone()))
        .collect();
    if tiles.is_empty() {
        tiles = kitsune_core::browser::QUICK_LINKS
            .iter()
            .map(|(l, u)| (String::from(kitsune_core::i18n::tr(l)), String::from(*u)))
            .collect();
    }
    // Recents belong to the window: every tab's history, the shown tab's first.
    let mut recents = t.browser.recent(6);
    for other in b.tabs.iter().filter(|o| o.id != t.id) {
        for u in other.browser.recent(6) {
            if recents.len() < 6 && !recents.contains(&u) {
                recents.push(u);
            }
        }
    }
    (tiles, recents)
}

/// Rows of the context menu of window `r` under `(cx, cy)`.
pub(crate) fn page_menu_row_at(m: &PageMenu, r: Rect, cx: i32, cy: i32) -> Option<usize> {
    let g = page_menu_geom(m, r);
    g.rows
        .iter()
        .zip(&m.items)
        .position(|(row, _)| row.contains(cx, cy))
}

/// Placement of the context menu, kept inside window `r`.
pub(crate) fn page_menu_geom(m: &PageMenu, r: Rect) -> kitsune_core::chrome::MenuGeom {
    let rows: Vec<kitsune_core::chrome::MenuRow> = m
        .items
        .iter()
        .map(|(_, label, _)| kitsune_core::chrome::MenuRow::Item {
            label_w: text::measure(label, text::BODY, text::Weight::Regular),
            shortcut_w: 0,
        })
        .collect();
    let mut g = kitsune_core::chrome::menu_geom(&rows, (m.x, m.y), r.right() + 4, r.bottom() + 4);
    // The menu must not leave the window: shift it back inside if it was clamped to the screen.
    let dx = (r.right() - 8 - g.rect.right()).min(0);
    let dy = (r.bottom() - 8 - g.rect.bottom()).min(0);
    if dx != 0 || dy != 0 {
        g.rect = Rect::new(g.rect.x + dx, g.rect.y + dy, g.rect.w, g.rect.h);
        for row in g.rows.iter_mut() {
            *row = Rect::new(row.x + dx, row.y + dy, row.w, row.h);
        }
    }
    g
}
