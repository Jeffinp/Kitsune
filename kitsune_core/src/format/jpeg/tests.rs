use super::*;
use crate::format::ppm;
use std::vec;

const S444: &[u8] = include_bytes!("testdata/s444.jpg");
const S420: &[u8] = include_bytes!("testdata/s420.jpg");
const S422: &[u8] = include_bytes!("testdata/s422.jpg");
const S440: &[u8] = include_bytes!("testdata/s440.jpg");
const GRAY: &[u8] = include_bytes!("testdata/gray.jpg");
const LOW: &[u8] = include_bytes!("testdata/low.jpg");
const ONE: &[u8] = include_bytes!("testdata/one.jpg");
const Q100: &[u8] = include_bytes!("testdata/q100.jpg");
const RESTART: &[u8] = include_bytes!("testdata/restart.jpg");
const PROGRESSIVE: &[u8] = include_bytes!("testdata/progressive.jpg");
const CMYK: &[u8] = include_bytes!("testdata/cmyk.jpg");

/// What libjpeg-turbo 2.1.5 produced from the same file with chroma replication (no smoothing).
fn expected(ppm_bytes: &[u8]) -> Image {
    ppm::decode(ppm_bytes).unwrap()
}

/// (max channel difference, mean channel difference x 1000)
fn diff(a: &Image, b: &Image) -> (i32, i32) {
    assert_eq!((a.width(), a.height()), (b.width(), b.height()));
    let (mut max, mut sum) = (0i32, 0i64);
    for (&p, &q) in a.pixels().iter().zip(b.pixels()) {
        for sh in [0, 8, 16] {
            let d = (((p >> sh) & 255) as i32 - ((q >> sh) & 255) as i32).abs();
            max = max.max(d);
            sum += d as i64;
        }
    }
    (max, (sum * 1000 / (a.pixels().len() as i64 * 3)) as i32)
}

fn check(jpg: &[u8], ppm_bytes: &[u8], name: &str) {
    let got = decode(jpg).unwrap_or_else(|e| panic!("{name}: {e}"));
    let (max, mean) = diff(&got, &expected(ppm_bytes));
    assert!(max <= 3, "{name}: max channel difference {max}");
    assert!(mean <= 800, "{name}: mean channel difference {mean}/1000");
    assert!(got.is_opaque());
}

#[test]
fn matches_libjpeg_for_every_sampling() {
    check(S444, include_bytes!("testdata/s444.ppm"), "4:4:4");
    check(S420, include_bytes!("testdata/s420.ppm"), "4:2:0");
    check(S422, include_bytes!("testdata/s422.ppm"), "4:2:2");
    check(S440, include_bytes!("testdata/s440.ppm"), "4:4:0");
}

#[test]
fn gray_and_one_pixel_and_exact_blocks_and_quality_extremes() {
    check(GRAY, include_bytes!("testdata/gray.ppm"), "gray");
    check(ONE, include_bytes!("testdata/one.ppm"), "1x1");
    check(Q100, include_bytes!("testdata/q100.ppm"), "16x16 q100");
    check(LOW, include_bytes!("testdata/low.ppm"), "q30");
}

#[test]
fn restart_intervals() {
    check(RESTART, include_bytes!("testdata/restart.ppm"), "restart");
    // The file really has a DRI marker and RST markers between the intervals.
    assert!(RESTART.windows(2).any(|w| w == [0xFF, 0xDD]));
    assert!(
        RESTART
            .windows(2)
            .any(|w| w[0] == 0xFF && (0xD0..=0xD7).contains(&w[1]))
    );
}

#[test]
fn dimensions_without_decoding() {
    for (j, wh) in [
        (S444, (37, 21)),
        (S420, (37, 21)),
        (GRAY, (37, 21)),
        (ONE, (1, 1)),
        (Q100, (16, 16)),
        (PROGRESSIVE, (37, 21)),
    ] {
        assert_eq!(peek_dims(j), Some(wh));
    }
    assert_eq!(peek_dims(b"GIF89a"), None);
    assert_eq!(peek_dims(&S444[..20]), None);
    assert_eq!(peek_dims(&[]), None);
}

#[test]
fn what_is_not_supported_is_said() {
    assert_eq!(
        decode(CMYK).unwrap_err(),
        JpegError::Unsupported(Feature::Components)
    );
    // 12-bit precision: patch the SOF0 precision byte.
    let mut v = S444.to_vec();
    let sof = v.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    v[sof + 4] = 12;
    assert_eq!(
        decode(&v).unwrap_err(),
        JpegError::Unsupported(Feature::Precision)
    );
    // Arithmetic and lossless frame markers.
    let mut v = S444.to_vec();
    v[sof + 1] = 0xC9;
    assert_eq!(
        decode(&v).unwrap_err(),
        JpegError::Unsupported(Feature::Arithmetic)
    );
    v[sof + 1] = 0xC3;
    assert_eq!(
        decode(&v).unwrap_err(),
        JpegError::Unsupported(Feature::Lossless)
    );
}

#[test]
fn errors() {
    assert_eq!(decode(b"").unwrap_err(), JpegError::BadSignature);
    assert_eq!(decode(b"\xFF\xD8").unwrap_err(), JpegError::BadSignature);
    assert_eq!(
        decode(b"\xFF\xD8\xFF\xD9").unwrap_err(),
        JpegError::Truncated,
        "no frame, no scan"
    );
    // The headers only, no entropy data: a frame but no scan.
    let sos = S444.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
    assert_eq!(decode(&S444[..sos]).unwrap_err(), JpegError::Truncated);
    // Zero width.
    let mut v = S444.to_vec();
    let sof = v.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    v[sof + 7] = 0;
    v[sof + 8] = 0;
    assert_eq!(decode(&v).unwrap_err(), JpegError::BadSize);
    // A second frame header.
    let mut v = S444[..sos].to_vec();
    v.extend_from_slice(&S444[sof..sos]);
    v.extend_from_slice(&S444[sos..]);
    assert!(matches!(
        decode(&v).unwrap_err(),
        JpegError::BadMarker(0xC0)
    ));
}

#[test]
fn a_huge_declared_size_is_refused_before_allocating() {
    let mut v = S444.to_vec();
    let sof = v.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
    // 65535 x 65535.
    v[sof + 5..sof + 9].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    assert_eq!(decode(&v).unwrap_err(), JpegError::BadSize);
}

#[test]
fn a_cut_off_file_decodes_as_far_as_it_goes() {
    let mut partial = 0;
    for cut in (0..S420.len()).step_by(3) {
        if let Ok(img) = decode(&S420[..cut]) {
            assert_eq!((img.width(), img.height()), (37, 21));
            partial += 1;
        }
    }
    assert!(
        partial > 10,
        "most late cuts still give a picture ({partial})"
    );
    // Cutting the entropy data in half gives the top of the picture and gray below.
    let sos = S420.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
    let mid = sos + (S420.len() - sos) / 2;
    let img = decode(&S420[..mid]).unwrap();
    let last = img.pixels()[img.pixels().len() - 1];
    assert_eq!(
        last,
        rgba(128, 128, 128, 255),
        "undecoded area stays mid-gray"
    );
}

#[test]
fn damaged_entropy_data_never_panics_or_hangs() {
    // Flip every byte of the entropy data in turn, to several values.
    let sos = S420.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
    for i in sos..S420.len() {
        for v in [0x00u8, 0xFF, 0x55, 0x80] {
            let mut m = S420.to_vec();
            m[i] = v;
            let _ = decode(&m);
        }
    }
    for i in 0..sos {
        let mut m = S420.to_vec();
        m[i] ^= 0xA5;
        let _ = decode(&m);
    }
}

#[test]
fn restart_data_damage_is_survivable() {
    for i in 0..RESTART.len() {
        let mut m = RESTART.to_vec();
        m[i] ^= 0xFF;
        let _ = decode(&m);
    }
}

#[test]
fn huffman_table_validation() {
    // A count claiming more codes of a length than fit.
    let mut counts = [0u8; 16];
    counts[0] = 3;
    assert!(Huff::new(&counts, &[0, 1, 2]).is_err());
    assert!(Huff::new(&[0; 16], &[]).is_err(), "an empty table");
    counts = [0; 16];
    counts[1] = 2;
    assert!(
        Huff::new(&counts, &[1]).is_err(),
        "symbol count does not match"
    );
    assert!(Huff::new(&counts, &[7, 9]).is_ok());
}

#[test]
fn the_idct_matches_a_float_reference() {
    // Compare the integer IDCT with the textbook formula on a few coefficient patterns.
    fn reference(c: &[i32; 64]) -> [u8; 64] {
        let mut out = [0u8; 64];
        for y in 0..8 {
            for x in 0..8 {
                let mut s = 0.0f64;
                for v in 0..8 {
                    for u in 0..8 {
                        let cu = if u == 0 { 1.0 / 2f64.sqrt() } else { 1.0 };
                        let cv = if v == 0 { 1.0 / 2f64.sqrt() } else { 1.0 };
                        s += cu
                            * cv
                            * c[v * 8 + u] as f64
                            * (((2 * x + 1) as f64 * u as f64 * core::f64::consts::PI) / 16.0)
                                .cos()
                            * (((2 * y + 1) as f64 * v as f64 * core::f64::consts::PI) / 16.0)
                                .cos();
                    }
                }
                out[y * 8 + x] = ((s / 4.0) + 128.0).round().clamp(0.0, 255.0) as u8;
            }
        }
        out
    }
    let mut seed = 7u32;
    let mut rnd = move || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 16) as i32
    };
    let mut worst = 0;
    for case in 0..200 {
        let mut c = [0i32; 64];
        c[0] = rnd() % 1600 - 800;
        for _ in 0..(case % 12) {
            c[(rnd() % 64) as usize] = rnd() % 200 - 100;
        }
        let a = idct::idct(&c);
        let b = reference(&c);
        for i in 0..64 {
            worst = worst.max((a[i] as i32 - b[i] as i32).abs());
        }
    }
    assert!(worst <= 2, "worst error {worst}");
    // A flat block is flat.
    let mut c = [0i32; 64];
    c[0] = 8 * 40;
    assert!(idct::idct(&c).iter().all(|&v| v == 128 + 40));
}

#[test]
fn extend_follows_the_spec() {
    assert_eq!(huffman::extend(0, 0), 0);
    assert_eq!(huffman::extend(0, 1), -1);
    assert_eq!(huffman::extend(1, 1), 1);
    assert_eq!(huffman::extend(0b000, 3), -7);
    assert_eq!(huffman::extend(0b011, 3), -4);
    assert_eq!(huffman::extend(0b100, 3), 4);
    assert_eq!(huffman::extend(0b111, 3), 7);
}

#[test]
fn the_signature_check() {
    assert!(is_jpeg(b"\xFF\xD8\xFF\xE0"));
    assert!(is_jpeg(b"\xFF\xD8\xFF"));
    assert!(!is_jpeg(b"\xFF\xD8"));
    assert!(!is_jpeg(b"GIF89a"));
    let _ = vec![0u8; 0];
}

// ---- progressive ----

const PROG_420: &[u8] = include_bytes!("testdata/prog_420.jpg");
const BASE_420: &[u8] = include_bytes!("testdata/base_420.jpg");
const PROG_422: &[u8] = include_bytes!("testdata/prog_422.jpg");
const BASE_422: &[u8] = include_bytes!("testdata/base_422.jpg");
const PROG_444: &[u8] = include_bytes!("testdata/prog_444.jpg");
const BASE_444: &[u8] = include_bytes!("testdata/base_444.jpg");
const PROG_GRAY: &[u8] = include_bytes!("testdata/prog_gray.jpg");
const BASE_GRAY: &[u8] = include_bytes!("testdata/base_gray.jpg");
const PROG_RST: &[u8] = include_bytes!("testdata/prog_rst.jpg");
const BASE_RST: &[u8] = include_bytes!("testdata/base_rst.jpg");

/// The coefficients of a progressive file are those of the sequential one made from the same
/// picture with the same tables, so the two must decode to exactly the same pixels.
#[test]
fn progressive_equals_sequential_for_every_sampling() {
    for (p, b, name) in [
        (PROG_420, BASE_420, "4:2:0"),
        (PROG_422, BASE_422, "4:2:2"),
        (PROG_444, BASE_444, "4:4:4"),
        (PROG_GRAY, BASE_GRAY, "gray"),
        (PROG_RST, BASE_RST, "restart intervals"),
    ] {
        assert!(
            p.windows(2).any(|w| w == [0xFF, 0xC2]),
            "{name} is progressive"
        );
        let (got, want) = (decode(p).unwrap(), decode(b).unwrap());
        assert_eq!(diff(&got, &want), (0, 0), "{name}: not identical");
    }
}

#[test]
fn progressive_matches_libjpeg_where_upsampling_is_exact() {
    check(
        PROG_444,
        include_bytes!("testdata/prog_444.ppm"),
        "progressive 4:4:4",
    );
    check(
        PROG_GRAY,
        include_bytes!("testdata/prog_gray.ppm"),
        "progressive gray",
    );
}

#[test]
fn a_progressive_file_cut_anywhere_decodes_as_far_as_it_goes() {
    // After the first scan the picture is already there (blurry); every later cut is at least as
    // good, and none panics.
    let first_ac = PROG_420
        .windows(2)
        .enumerate()
        .filter(|(_, w)| *w == [0xFF, 0xDA])
        .map(|(i, _)| i)
        .nth(1)
        .unwrap();
    let full = decode(PROG_420).unwrap();
    for n in (first_ac..PROG_420.len()).step_by(7) {
        let img = decode(&PROG_420[..n]).unwrap_or_else(|e| panic!("cut at {n}: {e}"));
        assert_eq!((img.width(), img.height()), (full.width(), full.height()));
    }
    // Only the DC scan: a recognisable, smooth version of the picture.
    let dc_only = decode(&PROG_420[..first_ac]).unwrap();
    let (max, mean) = diff(&dc_only, &full);
    assert!(max > 0 && mean < 30_000, "dc only: max {max} mean {mean}");
}

#[test]
fn progressive_scan_parameters_are_checked() {
    let scans: Vec<usize> = PROG_420
        .windows(2)
        .enumerate()
        .filter(|(_, w)| *w == [0xFF, 0xDA])
        .map(|(i, _)| i)
        .collect();
    // Patch each scan header's Ss/Se/AhAl with nonsense: an error or an image, never a panic.
    for &at in &scans {
        let len = u16::from_be_bytes([PROG_420[at + 2], PROG_420[at + 3]]) as usize;
        let tail = at + 2 + len - 3; // Ss, Se, Ah/Al
        for patch in [
            [64u8, 63, 0],
            [5, 3, 0],
            [0, 5, 0],
            [1, 63, 0xFF],
            [0, 0, 0xEF],
            [63, 63, 0x01],
        ] {
            let mut v = PROG_420.to_vec();
            v[tail..tail + 3].copy_from_slice(&patch);
            let _ = decode(&v);
        }
    }
    // A DC-only band with several components in an AC scan is refused.
    let at = scans[1];
    let mut v = PROG_420.to_vec();
    v[at + 4] = 3; // Ns = 3 in an AC scan
    assert!(decode(&v).is_err(), "an AC scan with three components");
}

#[test]
fn damaged_progressive_data_never_panics_or_hangs() {
    let mut x = 0x2545_F491u32;
    for _ in 0..400 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        let mut v = PROG_422.to_vec();
        let i = 2 + (x as usize) % (v.len() - 2);
        v[i] ^= (x >> 24) as u8 | 1;
        let _ = decode(&v);
    }
}

#[test]
fn a_huge_progressive_frame_is_refused_up_front() {
    let mut v = PROG_420.to_vec();
    let sof = v.windows(2).position(|w| w == [0xFF, 0xC2]).unwrap();
    // 4000 x 4000 4:2:0: about 24 million samples, over the limit for coefficients in memory.
    v[sof + 5..sof + 9].copy_from_slice(&[0x0F, 0xA0, 0x0F, 0xA0]);
    assert_eq!(
        decode(&v).unwrap_err(),
        JpegError::Unsupported(Feature::Progressive)
    );
    // The size itself is checked first.
    v[sof + 5..sof + 9].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    assert_eq!(decode(&v).unwrap_err(), JpegError::BadSize);
}
