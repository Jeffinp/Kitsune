//! Lays out an HTML file with the browser engine and prints the display list as tab-separated lines
//! (`tools/render-page.py` turns it into a PNG): a way to look at real pages without booting the system.
//!
//! `cargo run --release -p kitsune_core --example render_page -- page.html [width] [zoom]`
use kitsune_core::web::imgcache::NoImages;
use kitsune_core::web::{Cmd, Doc, FixedAdvance, Layout};
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let html = std::fs::read(&args[1]).expect("read the page");
    let width: i32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(960);
    let zoom: u16 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(100);
    let t0 = std::time::Instant::now();
    let mut doc = Doc::parse(&html);
    eprintln!("parse: {:?}", t0.elapsed());
    // `--links`: print the style sheets the page asks for and stop. `--css FILE...` (after the
    // other arguments): add those files as external sheets, in order.
    if args.iter().any(|a| a == "--links") {
        for l in doc.linked_sheets() {
            println!("{l}");
        }
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--css") {
        for f in &args[i + 1..] {
            if let Ok(css) = std::fs::read(f) {
                let t = std::time::Instant::now();
                doc.add_sheet(&String::from_utf8_lossy(&css));
                eprintln!("sheet {f} ({} B): {:?}", css.len(), t.elapsed());
            }
        }
    }
    let m = FixedAdvance;
    let t = std::time::Instant::now();
    let page = doc.layout(&Layout {
        width,
        zoom,
        images: &NoImages,
        metrics: &m,
    });
    eprintln!("layout: {:?} ({} commands)", t.elapsed(), page.cmds.len());
    let out = std::io::stdout();
    let mut out = out.lock();
    let _ = writeln!(
        out,
        "page\t{}\t{}\t{}\t{}\t{}\t{}",
        width,
        page.height,
        page.background.0,
        page.background.1,
        page.background.2,
        page.cmds.len()
    );
    for c in &page.cmds {
        let _ = match c {
            Cmd::Rect {
                x,
                y,
                w,
                h,
                color,
                radius,
            } => writeln!(
                out,
                "rect\t{x}\t{y}\t{w}\t{h}\t{}\t{}\t{}\t{radius}",
                color.0, color.1, color.2
            ),
            Cmd::Border {
                x,
                y,
                w,
                h,
                widths,
                color,
                ..
            } => writeln!(
                out,
                "border\t{x}\t{y}\t{w}\t{h}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                color.0, color.1, color.2, widths[0], widths[1], widths[2], widths[3]
            ),
            Cmd::Text {
                x,
                y,
                w,
                h,
                text,
                color,
                font,
                deco,
                ..
            } => writeln!(
                out,
                "text\t{x}\t{y}\t{w}\t{h}\t{}\t{}\t{}\t{}\t{}\t{}\t{deco}\t{}",
                color.0,
                color.1,
                color.2,
                font.size,
                font.bold as u8,
                font.mono as u8,
                text.replace(['\t', '\n'], " ")
            ),
            Cmd::Image { x, y, w, h, .. } => writeln!(out, "image\t{x}\t{y}\t{w}\t{h}"),
        };
    }
}
