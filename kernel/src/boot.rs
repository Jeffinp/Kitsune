//! Animated boot splash drawn before the desktop starts: the deep night wallpaper,
//! the brand mark (the fox with its tails), the name and a slim progress bar. The mark and
//! the name fade in over the first 0.7 s and out over the last 0.4 s.

use crate::fb::{Canvas, Color, Corner};
use crate::text::{self, BODY, Weight};
use kitsune_core::Rect;
use kitsune_core::wallpaper::{PRESETS, lerp_rgb};

fn rgb24(v: u32) -> Color {
    Color::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Side of the brand mark on the splash.
const MARK: i32 = 232;

/// Renders the brand mark once, so the first splash frame does not pay for it (the kernel
/// logs how long this took).
pub fn prewarm() {
    let _ = crate::icons::surface(crate::icons::Icon::Halo, MARK);
}

/// Opacity (`0..=256`) of the mark and the name at `progress` of the splash: a quick fade
/// in, steady, a short fade out.
fn fade(progress: f32) -> u32 {
    const IN: f32 = 0.14; // 0.7 s of 5
    const OUT: f32 = 0.08; // 0.4 s of 5
    let p = progress.clamp(0.0, 1.0);
    let a = if p < IN {
        p / IN
    } else if p > 1.0 - OUT {
        (1.0 - p) / OUT
    } else {
        1.0
    };
    // Smoothstep, so both ends ease.
    let a = a * a * (3.0 - 2.0 * a);
    (a * 256.0) as u32
}

/// Renders the splash's background once: the dark scheme of the default wallpaper, gradient and
/// soft glows (the glows are the expensive part, so they are not redrawn every frame).
pub fn draw_splash_background(c: &mut Canvas) {
    let w = c.width();
    let h = c.height();

    // The dark scheme of the default wallpaper: gradient and soft glows.
    let sc = PRESETS[0].scheme(true);
    for y in 0..h {
        let t = ((y * 255) / h.max(1)) as u32;
        c.fill_rect(0, y, w, 1, rgb24(lerp_rgb(sc.top, sc.bottom, t)));
    }
    let lut = kitsune_core::raster::glow_lut();
    for b in sc.blobs.iter().filter(|b| b.alpha > 0 && b.r > 0) {
        let cx = (w as i64 * b.x as i64 / 1000) as i32;
        let cy = (h as i64 * b.y as i64 / 1000) as i32;
        let rad = (w as i64 * b.r as i64 / 1000) as i32;
        c.glow(cx, cy, rad, rgb24(b.color), b.alpha as u32, &lut);
    }
}

/// Draws what changes from frame to frame over [`draw_splash_background`]: the brand mark and the
/// name fading in and out, and the progress bar. `progress` in `0.0..=1.0` fills the bar, `accent`
/// is the colour it was started with (the settings, loaded meanwhile, may change `theme::accent`).
pub fn draw_splash(c: &mut Canvas, progress: f32, accent: Color) {
    let w = c.width();
    let h = c.height();
    let lut = kitsune_core::raster::glow_lut();

    // Brand mark with a soft glow, the name below it.
    let a = fade(progress);
    let cx = w as i32 / 2;
    let gy = h as i32 * 19 / 100;
    c.glow(cx, gy + MARK / 2, MARK * 2, accent, 70 * a / 256, &lut);
    c.blit_surface(
        crate::icons::surface(crate::icons::Icon::Halo, MARK),
        cx - MARK / 2,
        gy,
        a,
    );
    let name = Rect::new(0, gy + MARK + 8, w as i32, 48);
    text::draw_centered_a(
        c,
        name,
        "Kitsune",
        40,
        Weight::Semibold,
        Color::rgb(255, 255, 255),
        a as u16,
    );
    let sub = Rect::new(0, name.bottom() + 2, w as i32, 24);
    text::draw_centered_a(
        c,
        sub,
        "Sistema operacional",
        BODY,
        Weight::Regular,
        Color::rgb(0xC4, 0xC6, 0xE8),
        a as u16,
    );

    // Slim progress bar: translucent track, accent fill.
    let bar_w = (w as i32 / 4).max(220);
    let bar = Rect::new((w as i32 - bar_w) / 2, h as i32 * 74 / 100, bar_w, 6);
    c.fill_rrect(bar, 3, Corner::Circle, Color::rgb(255, 255, 255), 46);
    let p = progress.clamp(0.0, 1.0);
    let fill = ((bar_w as f32 * p) as i32).max(0);
    if fill >= 6 {
        c.fill_rrect(
            Rect::new(bar.x, bar.y, fill, bar.h),
            3,
            Corner::Circle,
            accent.lerp(Color::rgb(255, 255, 255), 60),
            256,
        );
    }
}
