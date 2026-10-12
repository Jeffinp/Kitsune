//! `render_svg FILE.svg OUT.ppm [width]`: renders an SVG with the engine's rasteriser over white.
use kitsune_core::format::svg;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");
    let w: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(400);
    let mut img = svg::render(&bytes, w).expect("render");
    img.flatten(0xffffffff);
    let mut out = format!("P6\n{} {}\n255\n", img.width(), img.height()).into_bytes();
    for p in img.pixels() {
        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    std::fs::write(&a[2], out).expect("write");
}
