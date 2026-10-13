//! Tests of the layout: proportional wrapping, styles, lists, blocks, hostile input.
//!
//! The metrics are [`FixedAdvance`]: at 16 px every character is 8 px wide, a
//! natural line is 19 px and the paragraph line-height (1.5) is 24 px, so the
//! expected numbers can be worked out by hand.

use super::imgcache::NoImages;
use super::*;

fn lay(html: &str, w: i32) -> Page {
    render(html.as_bytes(), w)
}

fn lay_zoom(html: &str, w: i32, zoom: u16) -> Page {
    Doc::parse(html.as_bytes()).layout(&Layout {
        width: w,
        zoom,
        images: &NoImages,
        metrics: &FixedAdvance,
    })
}

/// `(x, y, text)` of every text run.
fn runs(p: &Page) -> Vec<(i32, i32, String)> {
    p.cmds
        .iter()
        .filter_map(|c| match c {
            Cmd::Text { x, y, text, .. } => Some((*x, *y, text.clone())),
            _ => None,
        })
        .collect()
}

fn texts(p: &Page) -> Vec<String> {
    runs(p).into_iter().map(|r| r.2).collect()
}

fn run_named<'a>(p: &'a Page, name: &str) -> &'a Cmd {
    p.cmds
        .iter()
        .find(|c| matches!(c, Cmd::Text { text, .. } if text == name))
        .unwrap_or_else(|| panic!("no run {name:?} in {:?}", texts(p)))
}

fn font_of(p: &Page, name: &str) -> Font {
    match run_named(p, name) {
        Cmd::Text { font, .. } => *font,
        _ => unreachable!(),
    }
}

fn x_of(p: &Page, name: &str) -> i32 {
    match run_named(p, name) {
        Cmd::Text { x, .. } => *x,
        _ => unreachable!(),
    }
}

fn y_of(p: &Page, name: &str) -> i32 {
    match run_named(p, name) {
        Cmd::Text { y, .. } => *y,
        _ => unreachable!(),
    }
}

fn distinct_ys(p: &Page) -> Vec<i32> {
    let mut ys: Vec<i32> = runs(p).iter().map(|r| r.1).collect();
    ys.sort_unstable();
    ys.dedup();
    ys
}

fn body0(inner: &str) -> String {
    format!("<body style='margin:0'>{inner}</body>")
}

fn rect_of(p: &Page, color: Rgb) -> Option<(i32, i32, i32, i32)> {
    p.cmds.iter().find_map(|c| match c {
        Cmd::Rect {
            x,
            y,
            w,
            h,
            color: col,
            ..
        } if *col == color => Some((*x, *y, *w, *h)),
        _ => None,
    })
}

mod boxes;
mod budgets_hostility;
mod links;
mod media;
mod modern;
mod styles;
mod tables;
mod text_wrapping;
mod zoom;
