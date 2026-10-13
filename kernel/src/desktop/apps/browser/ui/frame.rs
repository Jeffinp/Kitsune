//! The browser window as a whole: toolbar, page area and what floats over it.

use crate::desktop::*;
use crate::text::{self, CALLOUT, Weight};
use kitsune_core::browser::Status;
use kitsune_core::t;

impl Desktop {
    pub(crate) fn draw_browser(&self, c: &mut Canvas, r: Rect, focused: bool, bs: &BrowserState) {
        let p = theme::pal();
        let ch = bs.chrome(r);
        let t = bs.tabs.active();
        let id = self.browser_id();
        let mix = id.map_or(if focused { 256 } else { 0 }, |i| {
            self.focus_mix(i, focused)
        });

        // ---- the toolbar: the same colour as the title bar above it (it covers its hairline)
        let active_bg = theme::solid(p.window_bg);
        let inactive_bg = if theme::dark() {
            Color::rgb(0x27, 0x27, 0x2A)
        } else {
            Color::rgb(0xEC, 0xEC, 0xF0)
        };
        let bar_bg = inactive_bg.lerp(active_bg, (mix * 255 / 256) as u16);
        let band = Rect::new(r.x, ch.toolbar.y - 1, r.w, ch.toolbar.h + 1);
        c.fill_rect(
            band.x.max(0) as usize,
            band.y.max(0) as usize,
            band.w.max(0) as usize,
            band.h.max(0) as usize,
            bar_bg,
        );
        self.draw_toolbar(c, &ch, bs, focused);
        let (sc, sa) = theme::tint(p.separator);
        if ch.strip.h == 0 {
            c.blend_rect(Rect::new(r.x, ch.toolbar.bottom() - 1, r.w, 1), sc, sa);
        } else {
            self.draw_tab_strip(c, &ch, bs);
            c.blend_rect(Rect::new(r.x, ch.strip.bottom() - 1, r.w, 1), sc, sa);
        }

        // ---- the page area ----
        let content = ch.content;
        let status = t.browser.status();
        if t.browser.is_home() {
            self.draw_start_page(c, content, bs, focused);
        } else if let Some(page) = &t.page {
            self.paint_web_page(c, page, content, t.scroll, bs);
            self.paint_videos(c, page, content, t.scroll, bs);
            if crate::trace::ON && t.trace_t0.get() != 0 {
                crate::trace::note(
                    "load to first paint",
                    t.trace_t0.replace(0),
                    page.cmds.len() as u64,
                );
            }
            self.draw_page_banner(c, content, t);
            self.draw_page_scrollbar(c, content, page, t);
        } else if status == Status::Error {
            self.draw_error_page(c, content, bs);
        } else {
            // A page on its way: a blank sheet with a word, and the bar says the rest.
            let bg = theme::surface();
            c.fill_rect(
                content.x.max(0) as usize,
                content.y.max(0) as usize,
                content.w.max(0) as usize,
                content.h.max(0) as usize,
                bg,
            );
            if t.browser.is_loading() {
                text::draw_centered(
                    c,
                    Rect::new(content.x, content.y + content.h / 3, content.w, 24),
                    t!("web.loading"),
                    CALLOUT,
                    Weight::Regular,
                    theme::solid(p.text_tertiary),
                );
            }
        }

        // ---- over the page ----
        if t.find.is_open() {
            self.draw_page_find_bar(c, content, bs, focused);
        }
        self.draw_flashes(c, content, bs);
        if bs.popover {
            self.draw_security_popover(c, r, &ch, bs);
        }
        if bs.shield_pop {
            self.draw_blocker_popover(c, r, &ch, bs);
        }
        self.draw_suggestions(c, &ch, bs, focused);
        if let Some(m) = &bs.ctx {
            self.draw_page_menu(c, r, m, bs);
        }
    }
}
