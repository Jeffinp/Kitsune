use super::*;

const SW: i32 = 1280;
const SH: i32 = 800;

// ---- work area & fit ----

#[test]
fn work_area_sits_between_the_panel_and_the_taskbar() {
    let w = work_area(SW, SH);
    let bar = crate::windowing::taskbar::layout(SW, SH, 9).panel;
    assert_eq!(w.y, crate::ui::style::PANEL_H);
    assert_eq!(w.bottom() + WORK_DOCK_GAP, bar.y);
    assert_eq!((w.x, w.right()), (0, SW));
    // BIOS 1280x720 too.
    let b = work_area(1280, 720);
    assert!(b.h > 400);
}

#[test]
fn work_area_on_a_tiny_screen_is_not_negative() {
    let w = work_area(100, 60);
    assert!(w.w >= 0 && w.h >= 0);
}

#[test]
fn fit_scale_picks_the_largest_that_fits() {
    // Terminal-like grid: 240 x 135 at scale 1.
    assert_eq!(fit_scale(488, 282, 240, 135, 2, 4), 2);
    assert_eq!(fit_scale(740, 420, 240, 135, 2, 4), 3);
    assert_eq!(fit_scale(2000, 2000, 240, 135, 2, 4), 4); // capped
    // Width limits even when height would allow more.
    assert_eq!(fit_scale(500, 2000, 240, 135, 2, 4), 2);
}

#[test]
fn fit_scale_falls_back_to_base_when_nothing_fits() {
    assert_eq!(fit_scale(10, 10, 240, 135, 2, 4), 2);
    assert_eq!(fit_scale(0, 0, 240, 135, 2, 4), 2);
}

// ---- context menu ----

// ---- dock ----

// ---- calculator ----

#[test]
fn calc_keypad_maps_every_cell() {
    let r = Rect::new(100, 100, 320, 520);
    let g = calc_geom(r);
    for (row, cells) in g.keys.iter().enumerate() {
        for (col, cell) in cells.iter().enumerate() {
            assert!(cell.w > 0 && cell.h > 0, "row {row} col {col}");
            assert_eq!(
                calc_button_at(r, cell.x + cell.w / 2, cell.y + cell.h / 2),
                Some(CALC_KEYS[row][col]),
                "row {row} col {col}"
            );
        }
    }
}

#[test]
fn calc_gaps_and_outside_are_misses() {
    let r = Rect::new(100, 100, 320, 520);
    let g = calc_geom(r);
    let k = g.keys[2][1];
    assert_eq!(calc_button_at(r, k.right(), k.y), None); // horizontal gap
    assert_eq!(calc_button_at(r, k.x, k.bottom()), None); // vertical gap
    assert_eq!(calc_button_at(r, g.keys[2][0].x - 1, k.y), None);
    assert_eq!(calc_button_at(r, r.x + 2, r.bottom() - 2), None);
}

#[test]
fn calc_regions_stack_inside_the_window() {
    let r = Rect::new(0, 0, 320, 520);
    let g = calc_geom(r);
    assert!(g.history.y >= TITLE_H);
    assert_eq!(g.display.y, g.history.bottom());
    assert!(g.keys[0][0].y > g.display.bottom());
    assert!(g.keys[5][3].bottom() <= r.bottom());
    // The memory row is the short one and the rest are equal.
    assert_eq!(g.keys[0][0].h, CALC_MEM_H);
    assert!(g.keys[1][0].h > CALC_MEM_H);
    assert!(g.keys[1..].iter().all(|row| row[0].h == g.keys[1][0].h));
    // Copy sits in the history strip; clicking the display copies too.
    assert_eq!(calc_hit(r, g.copy.x + 2, g.copy.y + 2), Some(CalcHit::Copy));
    assert_eq!(
        calc_hit(r, g.display.x + 10, g.display.y + 10),
        Some(CalcHit::Copy)
    );
}

#[test]
fn calc_operators_are_in_the_last_column() {
    for (row, expect) in [(1, b'/'), (2, b'*'), (3, b'-'), (4, b'+'), (5, b'=')] {
        assert_eq!(CALC_KEYS[row][3], expect);
    }
}

#[test]
fn calc_degenerate_window_has_no_buttons() {
    let tiny = Rect::new(0, 0, 40, 40);
    assert_eq!(calc_button_at(tiny, 20, 20), None);
    let negative = Rect::new(0, 0, 10, 10);
    assert_eq!(calc_button_at(negative, 5, 5), None);
    // Even a tiny window never yields a negative-sized key.
    let g = calc_geom(tiny);
    assert!(g.keys.iter().flatten().all(|k| k.w >= 0 && k.h >= 0));
}

// ---- browser ----

#[test]
fn browser_chrome_toolbar_is_ordered_and_inside_the_window() {
    let r = Rect::new(80, 60, 700, 500);
    let c = BrowserChrome::of(r, 1, 0);
    assert!(c.back.right() <= c.forward.x);
    assert!(c.forward.right() < c.reload.x);
    assert!(c.reload.right() < c.bar.x);
    assert!(c.bar.right() < c.newtab.x);
    assert!(c.star.x >= c.bar.x && c.star.right() <= c.bar.right());
    assert_eq!(c.newtab.right(), r.right() - 12);
    assert_eq!(c.toolbar.y, r.y + TITLE_H);
    assert_eq!(c.toolbar.h, BROWSER_TOOLBAR_H);
    // Every control is vertically centred in the toolbar and on the 4 px grid.
    for b in [c.back, c.forward, c.reload, c.bar, c.newtab] {
        assert_eq!(b.y - c.toolbar.y, (BROWSER_TOOLBAR_H - b.h) / 2);
        assert_eq!(b.h % 4, 0);
    }
    // The page is edge to edge below the chrome.
    assert_eq!(c.content.x, r.x);
    assert_eq!(c.content.w, r.w);
    assert_eq!(c.content.y, c.toolbar.bottom());
    assert_eq!(c.content.bottom(), r.bottom());
    assert!(c.progress.y > c.bar.bottom() && c.progress.bottom() <= c.toolbar.bottom());
}

#[test]
fn the_tab_strip_appears_from_two_tabs_and_pushes_the_page_down() {
    let r = Rect::new(0, 0, 700, 500);
    let one = BrowserChrome::of(r, 1, 0);
    let two = BrowserChrome::of(r, 2, 0);
    assert_eq!(one.strip.h, 0);
    assert_eq!(two.strip.h, BROWSER_STRIP_H);
    assert_eq!(two.content.y, one.content.y + BROWSER_STRIP_H);
    assert_eq!(two.strip.y, two.toolbar.bottom());
}

#[test]
fn the_shield_takes_room_from_the_address_text() {
    let r = Rect::new(0, 0, 700, 500);
    let none = BrowserChrome::of(r, 1, 0);
    let some = BrowserChrome::of(r, 1, 120);
    assert_eq!(none.shield.w, 0);
    assert_eq!(some.shield.w, 120);
    assert!(some.text_x() > none.text_x());
    assert!(some.shield.x >= some.bar.x && some.shield.right() <= some.bar.right());
    // Never wider than half the omnibox.
    let wide = BrowserChrome::of(Rect::new(0, 0, 400, 300), 1, 5000);
    assert!(wide.shield.w <= wide.bar.w / 2);
}

#[test]
fn the_blocker_button_sits_between_the_address_and_the_star() {
    let r = Rect::new(0, 0, 700, 500);
    let base = BrowserChrome::of(r, 1, 100);
    assert_eq!(base.blocker.w, 0);
    let c = base.with_blocker(40);
    assert_eq!(c.blocker.w, 40);
    assert!(c.blocker.right() < c.star.x && c.blocker.x > c.shield.right());
    assert!(c.blocker.x >= c.bar.x && c.blocker.bottom() <= c.bar.bottom());
    assert_eq!(c.text_right(true), c.blocker.x - 4);
    assert_eq!(base.text_right(true), base.star.x - 4);
    assert_eq!(base.text_right(false), base.bar.right() - 12);
    // Never more than a third of the omnibox, and the text never gets a negative width.
    let narrow = BrowserChrome::of(Rect::new(0, 0, 300, 300), 1, 5000).with_blocker(5000);
    assert!(narrow.blocker.w <= narrow.bar.w / 3);
    assert!(narrow.text_right(true) >= narrow.text_x());
    // The popover hangs under the button, inside the window.
    let win = Rect::new(0, 0, 700, 500);
    let p = browser_blocker_popover(c.blocker, c.bar, win, 200);
    assert!(p.x >= win.x + 8 && p.right() <= win.right() - 8);
    assert_eq!(p.y, c.bar.bottom() + 8);
    let tiny = browser_blocker_popover(c.blocker, c.bar, Rect::new(0, 0, 200, 300), 100);
    assert!(tiny.x >= 8 && tiny.w >= 120);
}

#[test]
fn tab_rects_share_the_strip_and_animate_with_their_weights() {
    let strip = Rect::new(0, 100, 700, 36);
    let full = browser_tab_rects(strip, &[256, 256, 256]);
    assert_eq!(full.len(), 3);
    for r in &full {
        assert_eq!(r.h, BROWSER_TAB_H);
        assert!(r.w >= BROWSER_TAB_MIN_W && r.w <= BROWSER_TAB_MAX_W);
        assert_eq!(r.y, strip.y + 4);
    }
    assert_eq!(full[1].x - full[0].right(), 4);
    // Eight tabs in a narrow strip bottom out at the minimum.
    let tiny = browser_tab_rects(Rect::new(0, 0, 300, 36), &[256; 8]);
    assert!(tiny.iter().all(|r| r.w == BROWSER_TAB_MIN_W));
    // A tab that is opening is narrower and the others close ranks around it.
    let half = browser_tab_rects(strip, &[256, 128, 256]);
    assert!(half[1].w < full[1].w);
    assert!(half[1].w > 0);
    let zero = browser_tab_rects(strip, &[256, 0, 256]);
    assert_eq!(zero[1].w, 0);
    assert_eq!(zero[2].x, zero[0].right() + 4);
    assert!(browser_tab_rects(strip, &[]).is_empty());
    // Two tabs in a wide strip stop at the maximum width.
    let wide = browser_tab_rects(Rect::new(0, 0, 1280, 36), &[256, 256]);
    assert!(wide.iter().all(|r| r.w == BROWSER_TAB_MAX_W));
}

#[test]
fn tab_hit_testing_and_close_button() {
    let rects = browser_tab_rects(Rect::new(0, 0, 700, 36), &[256, 256, 0]);
    assert_eq!(
        browser_tab_at(&rects, rects[1].x + 5, rects[1].y + 5),
        Some(1)
    );
    assert_eq!(browser_tab_at(&rects, 2, 2), None);
    // A closed (zero width) tab cannot be hit.
    assert_eq!(browser_tab_at(&rects, rects[2].x, rects[2].y + 5), None);
    let x = browser_tab_close(rects[0]);
    assert!(x.x >= rects[0].x && x.right() <= rects[0].right());
    assert!(x.y >= rects[0].y && x.bottom() <= rects[0].bottom());
}

#[test]
fn suggestion_rows_stack_under_the_bar() {
    let bar = Rect::new(100, 50, 400, 32);
    let panel = browser_suggest_panel(bar, 3);
    let r0 = browser_suggestion_row(bar, 0);
    let r1 = browser_suggestion_row(bar, 1);
    assert!(panel.x == bar.x && panel.w == bar.w && panel.y > bar.bottom());
    assert!(r0.x > panel.x && r0.right() < panel.right());
    assert!(r0.y > panel.y);
    assert_eq!(r1.y - r0.y, BROWSER_SUGGEST_ROW);
    assert!(browser_suggestion_row(bar, 2).bottom() < panel.bottom());
    assert_eq!(browser_suggestion_at(bar, 3, 120, r1.y + 3), Some(1));
    assert_eq!(browser_suggestion_at(bar, 1, 120, r1.y + 3), None);
    assert_eq!(browser_suggestion_at(bar, 3, 5, r0.y), None);
}

#[test]
fn popover_stays_inside_the_window() {
    let win = Rect::new(100, 100, 500, 400);
    let bar = Rect::new(380, 150, 200, 32);
    let p = browser_popover(bar, win, 200);
    assert!(p.x >= win.x && p.right() <= win.right());
    assert_eq!(p.y, bar.bottom() + 8);
    let narrow = browser_popover(bar, Rect::new(0, 0, 200, 300), 100);
    assert!(narrow.right() <= 200);
}

#[test]
fn find_bar_sits_in_the_top_right_and_its_parts_fit() {
    let content = Rect::new(0, 80, 700, 400);
    let f = browser_find_layout(content);
    assert!(f.bar.x >= content.x && f.bar.right() <= content.right());
    assert_eq!(f.bar.y, content.y + 12);
    for r in [f.field, f.count, f.prev, f.next, f.close] {
        assert!(r.x >= f.bar.x && r.right() <= f.bar.right(), "{r:?}");
        assert!(r.y >= f.bar.y && r.bottom() <= f.bar.bottom());
    }
    assert!(f.field.right() <= f.count.x);
    assert!(f.count.right() <= f.prev.x);
    assert!(f.prev.right() <= f.next.x && f.next.right() <= f.close.x);
    // Tiny windows do not push it out to the left.
    let t = browser_find_layout(Rect::new(0, 0, 150, 100));
    assert!(t.bar.x >= 0);
}

#[test]
fn error_page_parts_are_stacked_and_inside_the_page() {
    let content = Rect::new(0, 80, 800, 480);
    for cert in [false, true] {
        let e = browser_error_layout(content, cert);
        for r in [e.art, e.title, e.cause, e.retry] {
            assert!(r.x >= content.x && r.right() <= content.right(), "{r:?}");
            assert!(r.y >= content.y && r.bottom() <= content.bottom(), "{r:?}");
        }
        assert!(e.art.bottom() < e.title.y && e.title.bottom() <= e.cause.y);
        assert!(e.cause.bottom() < e.retry.y);
        assert_eq!(e.art.x + e.art.w / 2, content.x + content.w / 2);
        assert_eq!(e.retry.x + e.retry.w / 2, content.x + content.w / 2);
        if cert {
            assert!(e.proceed.y > e.retry.bottom());
            assert!(e.note.bottom() <= content.bottom());
        }
    }
}

#[test]
fn start_page_has_a_search_field_tiles_and_recents_that_fit() {
    let content = Rect::new(0, 80, 900, 480);
    let s = browser_start_layout(content, 8, 6);
    assert_eq!(s.search.x + s.search.w / 2, content.x + content.w / 2);
    assert!(s.search.h == 48);
    assert!(!s.tiles.is_empty() && s.tiles.len() <= 8);
    for t in s.tiles.iter().chain(&s.recents) {
        assert!(t.x >= content.x && t.right() <= content.right());
        assert!(t.y >= content.y && t.bottom() <= content.bottom(), "{t:?}");
    }
    // Tiles are in a grid: the second is right of the first, 16 px apart.
    assert_eq!(s.tiles[1].x - s.tiles[0].right(), 16);
    assert!(s.tiles_heading.bottom() <= s.tiles[0].y);
    assert!(s.recent_heading.y >= s.tiles.last().unwrap().bottom());
    // A short window drops what does not fit.
    let short = browser_start_layout(Rect::new(0, 0, 900, 260), 8, 6);
    assert!(short.tiles.len() < s.tiles.len() || short.recents.len() < s.recents.len());
    for t in short.tiles.iter().chain(&short.recents) {
        assert!(t.bottom() <= 260);
    }
    // Zero items, narrow window: still sane.
    let n = browser_start_layout(Rect::new(0, 0, 200, 300), 0, 0);
    assert!(n.tiles.is_empty() && n.recents.is_empty());
    assert!(n.search.x >= 0 && n.search.right() <= 200);
}

#[test]
fn zoom_pill_is_at_the_bottom_right() {
    let content = Rect::new(0, 80, 700, 400);
    let z = browser_zoom_pill(content);
    assert!(z.right() < content.right() && z.bottom() < content.bottom());
}

#[test]
fn browser_chrome_survives_tiny_windows() {
    let c = BrowserChrome::of(Rect::new(0, 0, 50, 50), 2, 80);
    assert_eq!(c.bar.w, 120); // the omnibox never collapses below 120
    assert_eq!(c.content.h, 0); // negative height clamps to empty
}

// ---- Apps launcher ----

// ---- file manager ----

#[test]
fn files_sidebar_zones() {
    let r = Rect::new(50, 50, 600, 400);
    let cy0 = r.y + TITLE_H;
    let x = r.x + 10;
    assert_eq!(files_hit(r, 0, x, cy0 + 34), Some(FilesHit::View(0)));
    assert_eq!(files_hit(r, 0, x, cy0 + 63), Some(FilesHit::View(0)));
    assert_eq!(files_hit(r, 0, x, cy0 + 64), None);
    assert_eq!(files_hit(r, 0, x, cy0 + 66), Some(FilesHit::View(1)));
    assert_eq!(files_hit(r, 0, x, cy0 + 132), Some(FilesHit::View(2)));
    assert_eq!(files_hit(r, 0, x, cy0 + 164), Some(FilesHit::View(3)));
    assert_eq!(files_hit(r, 0, x, cy0 + 194), None);
    assert_eq!(files_hit(r, 0, x, cy0 + 208), Some(FilesHit::View(4)));
    assert_eq!(files_hit(r, 0, x, cy0 + 237), Some(FilesHit::View(4)));
    assert_eq!(files_hit(r, 0, x, cy0 + 238), None);
    assert_eq!(files_hit(r, 0, x, cy0 + 10), None);
}

#[test]
fn files_rows_only_in_file_and_trash_views() {
    let r = Rect::new(50, 50, 600, 400);
    let list_y = r.y + TITLE_H + 78;
    let x = r.x + FILES_SIDEBAR_W + 5;
    assert_eq!(files_hit(r, 0, x, list_y), Some(FilesHit::Row(0)));
    assert_eq!(files_hit(r, 0, x, list_y + 29), Some(FilesHit::Row(0)));
    assert_eq!(files_hit(r, 1, x, list_y + 30), Some(FilesHit::Row(1)));
    assert_eq!(files_hit(r, 0, x, list_y + 95), Some(FilesHit::Row(3)));
    assert_eq!(files_hit(r, 0, x, list_y - 1), None); // header area
    assert_eq!(files_hit(r, 2, x, list_y), None); // disk view has no rows
    assert_eq!(files_hit(r, 3, x, list_y), None);
    assert_eq!(files_hit(r, 4, x, list_y), None); // the Apps view has its own rows
}

#[test]
fn files_sidebar_edge_belongs_to_the_main_list() {
    let r = Rect::new(0, 0, 600, 400);
    let list_y = TITLE_H + 78;
    assert_eq!(
        files_hit(r, 0, FILES_SIDEBAR_W - 1, TITLE_H + 40),
        Some(FilesHit::View(0))
    );
    assert_eq!(
        files_hit(r, 0, FILES_SIDEBAR_W, list_y),
        Some(FilesHit::Row(0))
    );
}
