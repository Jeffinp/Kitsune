//! The layout snapshot the page's scripts measure with (`getBoundingClientRect`, `offsetWidth`,
//! `getComputedStyle`, the observers: `user-apps/jsrun/src/bin/jsrun/layoutapi.js`).
//!
//! Sent after every layout of a page with a script process, as one binary message (the network data
//! tag with request id 0, [`data_message`]), so scripts read their measures synchronously from the
//! last layout. Little-endian 32-bit words: a header (version 1, viewport width and height, scroll x
//! and y, page width and height, all in CSS px, the zoom in percent, the record count), then one
//! record of [`RECORD_WORDS`] words per element: uid, x, y, width, height (CSS px, page coordinates:
//! the union of the element's own boxes), colour (0xRRGGBB), background (0xAARRGGBB, 0 = none),
//! `font px << 16 | display << 11 | flags` ([`crate::browsing::web::STYLE_DISPLAY`] and the `STYLE_*`
//! bits), margins (`top << 16 | right`, `bottom << 16 | left`, 16 bits each), paddings (same) and the
//! border widths (one byte each, top first). Cost: 52 bytes per element per layout (a page of 2000
//! elements: ~100 KiB), against the layout itself, which is far dearer.

use super::*;
use crate::browsing::web::{Page, STYLE_BOX, StyleRec};
use alloc::collections::BTreeMap;

/// Words per element record.
pub const RECORD_WORDS: usize = 13;
/// Most elements in one snapshot (the message stays well under the 4 MiB of a page message).
pub const MAX_BOX_RECORDS: usize = 60_000;
/// The request id the snapshot travels under.
pub const BOXES_ID: u32 = 0;

/// The snapshot of `page`, seen in `view` (see the module documentation).
pub fn boxes_message(page: &Page, view: &View) -> Vec<u8> {
    let zoom = i64::from(view.zoom.max(1));
    let css = |v: i32| (i64::from(v) * 100 / zoom) as i32;
    // The union of every box of each element.
    let mut boxes: BTreeMap<u32, (i32, i32, i32, i32)> = BTreeMap::new();
    for h in page.hits.iter().filter(|h| h.node != 0) {
        let (x1, y1) = (h.x.saturating_add(h.w), h.y.saturating_add(h.h));
        boxes
            .entry(h.node)
            .and_modify(|b| {
                b.0 = b.0.min(h.x);
                b.1 = b.1.min(h.y);
                b.2 = b.2.max(x1);
                b.3 = b.3.max(y1);
            })
            .or_insert((h.x, h.y, x1, y1));
    }
    let mut styles: BTreeMap<u32, StyleRec> = BTreeMap::new();
    for s in &page.styles {
        styles.insert(s.uid, *s);
    }
    let mut uids: Vec<u32> = boxes.keys().chain(styles.keys()).copied().collect();
    uids.sort_unstable();
    uids.dedup();
    uids.truncate(MAX_BOX_RECORDS);
    let mut words: Vec<i32> = Vec::with_capacity(9 + uids.len() * RECORD_WORDS);
    words.extend_from_slice(&[
        1,
        view.w,
        view.h,
        view.sx,
        view.sy,
        view.page_w,
        view.page_h,
        i32::from(view.zoom),
        uids.len() as i32,
    ]);
    let pair = |a: i16, b: i16| (i32::from(a) << 16) | (i32::from(b) & 0xffff);
    for uid in uids {
        let st = styles.get(&uid).copied().unwrap_or(StyleRec {
            uid,
            display: 2,
            font_px: 16,
            ..StyleRec::default()
        });
        let (x, y, w, h, has_box) = match boxes.get(&uid) {
            Some(&(x0, y0, x1, y1)) => (css(x0), css(y0), css(x1 - x0), css(y1 - y0), true),
            None => (0, 0, 0, 0, false),
        };
        let flags = st.flags | if has_box { STYLE_BOX } else { 0 };
        let packed = (u32::from(st.font_px) << 16)
            | (u32::from(st.display & 31) << 11)
            | u32::from(flags & 0x7ff);
        let border = u32::from_le_bytes(st.border);
        words.extend_from_slice(&[
            uid as i32,
            x,
            y,
            w,
            h,
            st.color as i32,
            st.bg as i32,
            packed as i32,
            pair(st.margin[0], st.margin[1]),
            pair(st.margin[2], st.margin[3]),
            pair(st.padding[0], st.padding[1]),
            pair(st.padding[2], st.padding[3]),
            border as i32,
        ]);
    }
    let mut bytes = Vec::with_capacity(words.len() * 4);
    for w in words {
        bytes.extend_from_slice(&w.to_le_bytes());
    }
    data_message(BOXES_ID, &bytes)
}
