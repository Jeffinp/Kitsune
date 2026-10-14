use super::*;
use alloc::vec;

/// A picture whose every byte differs from its neighbours.
fn picture(w: usize, h: usize) -> Vec<u8> {
    (0..w * h * 4).map(|i| (i * 7 + i / 13) as u8).collect()
}

/// The same copy, one pixel at a time (a division per pixel): the reference.
fn naive(
    s: &mut Surface,
    src: &[u8],
    (sw, sh): (usize, usize),
    src_bgr: bool,
    dst: Area,
    clip: Area,
) {
    let Some(r) = dst.intersection(&clip) else {
        return;
    };
    for y in r.y..r.y + r.h {
        let sy = ((y - dst.y) as usize * sh / dst.h as usize).min(sh - 1);
        for x in r.x..r.x + r.w {
            let sx = ((x - dst.x) as usize * sw / dst.w as usize).min(sw - 1);
            let i = (sy * sw + sx) * 4;
            let (red, green, blue) = if src_bgr {
                (src[i + 2], src[i + 1], src[i])
            } else {
                (src[i], src[i + 1], src[i + 2])
            };
            let o = (y as usize * s.stride + x as usize) * s.bpp;
            let px = if s.bgr {
                [blue, green, red]
            } else {
                [red, green, blue]
            };
            s.buf[o..o + 3].copy_from_slice(&px);
            if s.bpp == 4 {
                s.buf[o + 3] = src[i + 3];
            }
        }
    }
}

#[test]
fn the_row_copy_equals_the_pixel_by_pixel_copy() {
    let (fw, fh) = (300usize, 200usize);
    let cases = [
        // (picture size, box, clip)
        (
            (160, 90),
            Area {
                x: 10,
                y: 5,
                w: 160,
                h: 90,
            },
            Area {
                x: 0,
                y: 0,
                w: 300,
                h: 200,
            },
        ),
        (
            (160, 90),
            Area {
                x: 3,
                y: 7,
                w: 285,
                h: 160,
            },
            Area {
                x: 0,
                y: 0,
                w: 300,
                h: 200,
            },
        ),
        (
            (160, 120),
            Area {
                x: 0,
                y: 0,
                w: 80,
                h: 60,
            },
            Area {
                x: 0,
                y: 0,
                w: 300,
                h: 200,
            },
        ),
        (
            (107, 60),
            Area {
                x: 20,
                y: 20,
                w: 214,
                h: 120,
            },
            Area {
                x: 50,
                y: 40,
                w: 90,
                h: 70,
            },
        ),
        (
            (64, 48),
            Area {
                x: 250,
                y: 150,
                w: 128,
                h: 96,
            },
            Area {
                x: 0,
                y: 0,
                w: 300,
                h: 200,
            },
        ),
    ];
    for bpp in [3usize, 4] {
        for (src_bgr, fb_bgr) in [(true, true), (false, true), (true, false), (false, false)] {
            for &((sw, sh), dst, clip) in &cases {
                let src = picture(sw, sh);
                let mut a = vec![0x55u8; fw * fh * bpp];
                let mut b = a.clone();
                let mut sa = Surface {
                    buf: &mut a,
                    stride: fw,
                    bpp,
                    bgr: fb_bgr,
                };
                assert!(blit_scaled(
                    &mut sa,
                    &src,
                    (sw, sh),
                    src_bgr,
                    dst,
                    clip,
                    &plain_copy
                ));
                let mut sb = Surface {
                    buf: &mut b,
                    stride: fw,
                    bpp,
                    bgr: fb_bgr,
                };
                naive(&mut sb, &src, (sw, sh), src_bgr, dst, clip);
                if bpp == 3 {
                    assert_eq!(
                        a, b,
                        "bpp {bpp} src_bgr {src_bgr} fb_bgr {fb_bgr} {sw}x{sh} -> {dst:?} in {clip:?}"
                    );
                } else {
                    // 4 bytes a pixel: the fourth byte is the picture's (alpha), the same in both.
                    assert_eq!(
                        a, b,
                        "bpp {bpp} src_bgr {src_bgr} fb_bgr {fb_bgr} {sw}x{sh} -> {dst:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn odd_surfaces_and_short_pictures_are_refused() {
    let mut buf = vec![0u8; 100 * 100 * 2];
    let area = Area {
        x: 0,
        y: 0,
        w: 10,
        h: 10,
    };
    let mut s = Surface {
        buf: &mut buf,
        stride: 100,
        bpp: 2,
        bgr: true,
    };
    assert!(!blit_scaled(
        &mut s,
        &picture(10, 10),
        (10, 10),
        true,
        area,
        area,
        &plain_copy
    ));
    let mut buf = vec![0u8; 100 * 100 * 4];
    let mut s = Surface {
        buf: &mut buf,
        stride: 100,
        bpp: 4,
        bgr: true,
    };
    assert!(!blit_scaled(
        &mut s,
        &picture(10, 9),
        (10, 10),
        true,
        area,
        area,
        &plain_copy
    ));
    // Nothing of the box inside the clip: nothing drawn, nothing wrong.
    let far = Area {
        x: 50,
        y: 50,
        w: 10,
        h: 10,
    };
    assert!(blit_scaled(
        &mut s,
        &picture(10, 10),
        (10, 10),
        true,
        area,
        far,
        &plain_copy
    ));
    assert!(buf.iter().all(|&b| b == 0));
}

/// What a picture costs on this machine: `cargo test --release -p kitsune_core videoblit_speed --
/// --ignored --nocapture`. The figures are in docs/design/tls-browser.md (the arithmetic of 60 fps).
#[test]
#[ignore]
fn videoblit_speed() {
    extern crate std;
    use std::time::Instant;
    for (name, (sw, sh), (dw, dh)) in [
        (
            "240p (4:3) in 853x480",
            (320usize, 240usize),
            (640i32, 480i32),
        ),
        ("360p in 853x480", (640, 360), (853, 480)),
        ("720p in 1280x720 (1:1)", (1280, 720), (1280, 720)),
        ("360p to 1280x720", (640, 360), (1280, 720)),
        ("1080p to 1280x720", (1920, 1080), (1280, 720)),
    ] {
        for bpp in [3usize, 4] {
            let src = picture(sw, sh);
            let mut buf = vec![0u8; 1280 * 720 * bpp];
            let dst = Area {
                x: 0,
                y: 0,
                w: dw,
                h: dh,
            };
            let clip = Area {
                x: 0,
                y: 0,
                w: 1280,
                h: 720,
            };
            let n = 200;
            let t0 = Instant::now();
            for _ in 0..n {
                let mut s = Surface {
                    buf: &mut buf,
                    stride: 1280,
                    bpp,
                    bgr: true,
                };
                blit_scaled(&mut s, &src, (sw, sh), true, dst, clip, &plain_copy);
            }
            let us = t0.elapsed().as_micros() as f64 / f64::from(n);
            std::println!(
                "videoblit {name}, {bpp} bytes a pixel: {:.2} ms a picture",
                us / 1000.0
            );
        }
    }
}
