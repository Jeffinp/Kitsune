//! Laying a tab's page out and keeping its scroll in range.

use crate::desktop::*;
use kitsune_core::web::imgcache::{ImageCache, PageImages, image_key};
use kitsune_core::web::{Cmd as WebCmd, Layout};

/// Put tab `t` at scroll `y` without easing.
pub(super) fn jump_scroll(t: &mut TabData, y: i32) {
    t.scroll = y;
    t.scroll_spring.jump(y as f32);
}

/// Keep the scroll of `t` inside `0..=max` (after the page got shorter).
pub(super) fn clamp_scroll(t: &mut TabData, max: i32) {
    let target = (t.scroll_spring.target() as i32).clamp(0, max);
    t.scroll_spring.set_target(target as f32);
    if t.scroll > max {
        t.scroll = max;
        t.scroll_spring.jump(target as f32);
    }
}

/// Lay the document of `t` out for `width` pixels with its zoom and whatever the image cache
/// knows. `register` (a new page) first lays out with the images unknown to learn which
/// pictures the page has, asks the cache for them, then lays out again.
pub(crate) fn layout_browser(t: &mut TabData, images: &mut ImageCache, width: i32, register: bool) {
    let Some(doc) = &t.doc else {
        return;
    };
    let base = t.browser.nav_url().to_vec();
    let zoom = t.zoom;
    let lay = |images: &ImageCache| {
        doc.layout(&Layout {
            width,
            zoom,
            images: &PageImages {
                cache: images,
                base: &base,
            },
            metrics: &apps::browser::paint::KernelMetrics,
        })
    };
    let started = crate::interrupts::ticks();
    let t0 = crate::trace::t();
    let mut page = lay(images);
    crate::trace::note("layout", t0, page.cmds.len() as u64);
    if register {
        t.forms = kitsune_core::web::form::FormState::new(&page.forms);
        t.img_keys.clear();
        for r in &page.images {
            let key = image_key(&base, &r.src);
            if let Some(k) = &key {
                let data = (k.starts_with("data:#")).then_some(r.src.as_str());
                images.want(k, data);
            }
            t.img_keys.push(key);
        }
        if page.images.len() > kitsune_core::web::imgcache::MAX_PAGE_IMAGES {
            page = lay(images);
        }
    }
    // Make each stored picture exactly the size of its box so painting is a plain copy.
    for c in &page.cmds {
        if let WebCmd::Image { w, h, idx, .. } = c
            && let Some(Some(k)) = t.img_keys.get(*idx)
        {
            images.fit_to(k, *w as usize, *h as usize);
        }
    }
    t.page = Some(page);
    t.relayout.done(started, crate::interrupts::ticks());
    // The text moved: the selection and the find matches follow the new layout.
    t.find
        .refresh(t.page.as_ref(), &apps::browser::paint::KernelMetrics);
    if let Some(p) = &t.page
        && t.sel.is_some_and(|sel| !p.selection_valid(&sel))
    {
        t.sel = None;
    }
}
