use super::*;
use alloc::vec;
use alloc::vec::Vec;

fn file(rate: u32, ch: u16, bits: u16, data: &[u8]) -> Vec<u8> {
    let mut f = header(rate, ch, bits, data.len() as u32).to_vec();
    f.extend_from_slice(data);
    f
}

#[test]
fn reads_16_bit_stereo() {
    let data: Vec<u8> = [1i16, -2, 300, -32768]
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect();
    let f = file(44_100, 2, 16, &data);
    let w = parse(&f).unwrap();
    assert_eq!(
        (w.rate, w.channels, w.format),
        (44_100, 2, SampleFormat::Int(16))
    );
    assert_eq!(w.frames(), 2);
    let mut out = [0i16; 8];
    assert_eq!(w.decode_s16(0, &mut out), 2);
    assert_eq!(&out[..4], &[1, -2, 300, -32768]);
    assert_eq!(w.decode_s16(1, &mut out), 1);
    assert_eq!(&out[..2], &[300, -32768]);
    assert_eq!(w.decode_s16(2, &mut out), 0);
}

#[test]
fn eight_bit_is_unsigned() {
    let f = file(8000, 1, 8, &[128, 255, 0]);
    let w = parse(&f).unwrap();
    let mut out = [0i16; 3];
    assert_eq!(w.decode_s16(0, &mut out), 3);
    assert_eq!(out, [0, 127 << 8, -32768]);
    assert_eq!(w.duration_ms(), 0);
}

#[test]
fn wide_and_float_samples() {
    // 24-bit: 0x123456 -> top 16 bits 0x1234.
    let f = file(48_000, 1, 24, &[0x56, 0x34, 0x12, 0x00, 0x00, 0x80]);
    let w = parse(&f).unwrap();
    let mut out = [0i16; 2];
    w.decode_s16(0, &mut out);
    assert_eq!(out, [0x1234, i16::MIN]);
    // Float: header written by hand (format 3).
    let mut f = file(48_000, 1, 32, &[]);
    f[20] = 3;
    for v in [0.5f32, -1.0, 2.0, 0.0] {
        f.extend_from_slice(&v.to_le_bytes());
    }
    let len = (f.len() - 44) as u32;
    f[40..44].copy_from_slice(&len.to_le_bytes());
    let w = parse(&f).unwrap();
    assert_eq!(w.format, SampleFormat::Float32);
    let mut out = [9i16; 4];
    assert_eq!(w.decode_s16(0, &mut out), 4);
    assert_eq!(out, [16384, -32768, 32767, 0]);
}

#[test]
fn float_conversion_matches_reference() {
    for v in [
        0.0f32, 1e-9, 0.25, -0.25, 0.999, -0.999, 1.0, -1.0, 7.0, -7.0, 3.1e-5,
    ] {
        let want = (v * 32768.0).clamp(-32768.0, 32767.0) as i16;
        assert_eq!(float_to_s16(v.to_bits()), want, "{v}");
    }
    assert_eq!(float_to_s16(f32::NAN.to_bits()), 0);
    assert_eq!(float_to_s16(f32::INFINITY.to_bits()), i16::MAX);
    assert_eq!(float_to_s16(f32::NEG_INFINITY.to_bits()), i16::MIN);
}

#[test]
fn skips_other_chunks_and_padding() {
    let mut f = Vec::new();
    f.extend_from_slice(b"RIFF\0\0\0\0WAVE");
    f.extend_from_slice(b"LIST");
    f.extend_from_slice(&3u32.to_le_bytes());
    f.extend_from_slice(&[1, 2, 3, 0]); // odd size + pad byte
    f.extend_from_slice(&header(22_050, 1, 16, 2)[12..36]);
    f.extend_from_slice(b"data");
    f.extend_from_slice(&4u32.to_le_bytes());
    f.extend_from_slice(&[0x10, 0, 0x20, 0]);
    let w = parse(&f).unwrap();
    assert_eq!(w.rate, 22_050);
    assert_eq!(w.data, &[0x10, 0, 0x20, 0]);
}

#[test]
fn extensible_pcm() {
    let mut f = Vec::new();
    f.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
    f.extend_from_slice(&40u32.to_le_bytes());
    f.extend_from_slice(&0xFFFEu16.to_le_bytes());
    f.extend_from_slice(&2u16.to_le_bytes());
    f.extend_from_slice(&48_000u32.to_le_bytes());
    f.extend_from_slice(&192_000u32.to_le_bytes());
    f.extend_from_slice(&4u16.to_le_bytes());
    f.extend_from_slice(&16u16.to_le_bytes());
    f.extend_from_slice(&22u16.to_le_bytes()); // cbSize
    f.extend_from_slice(&16u16.to_le_bytes()); // valid bits
    f.extend_from_slice(&3u32.to_le_bytes()); // mask
    f.extend_from_slice(&1u16.to_le_bytes()); // GUID starts with the PCM tag
    f.extend_from_slice(&[0; 14]);
    f.extend_from_slice(b"data");
    f.extend_from_slice(&4u32.to_le_bytes());
    f.extend_from_slice(&[1, 0, 2, 0]);
    let w = parse(&f).unwrap();
    assert_eq!(
        (w.channels, w.format, w.frames()),
        (2, SampleFormat::Int(16), 1)
    );
}

#[test]
fn truncated_data_and_partial_frames() {
    let mut f = file(16_000, 2, 16, &[1, 0, 2, 0, 3, 0, 4, 0, 5]);
    // Claim far more than present: cut to what is there, then to whole frames.
    f[40..44].copy_from_slice(&1_000_000u32.to_le_bytes());
    let w = parse(&f).unwrap();
    assert_eq!(w.data.len(), 8);
    assert_eq!(w.frames(), 2);
}

#[test]
fn rejects_bad_files() {
    assert_eq!(parse(b"RIFX\0\0\0\0WAVE"), Err(WavError::NotWav));
    assert_eq!(parse(b""), Err(WavError::NotWav));
    assert_eq!(parse(b"RIFF\0\0\0\0WAVE"), Err(WavError::NoFormat));
    let ok = file(8000, 1, 16, &[0, 0]);
    let mut no_data = ok.clone();
    no_data[36..40].copy_from_slice(b"junk");
    assert_eq!(parse(&no_data), Err(WavError::NoData));
    let mut adpcm = ok.clone();
    adpcm[20] = 2;
    assert_eq!(parse(&adpcm), Err(WavError::Unsupported));
    let mut six = file(8000, 6, 16, &[0; 12]);
    six[32] = 12;
    assert_eq!(parse(&six), Err(WavError::Channels));
    let mut zero_rate = ok.clone();
    zero_rate[24..28].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(parse(&zero_rate), Err(WavError::BadHeader));
    let mut bad_align = ok.clone();
    bad_align[32] = 3;
    assert_eq!(parse(&bad_align), Err(WavError::BadHeader));
    // A data chunk before fmt.
    let mut early = Vec::new();
    early.extend_from_slice(b"RIFF\0\0\0\0WAVEdata\x02\0\0\0\0\0");
    assert_eq!(parse(&early), Err(WavError::NoFormat));
}

#[test]
fn garbage_never_panics() {
    // A small deterministic fuzz: random bytes after a valid prefix.
    let mut x = 0x1234_5678u32;
    for len in 0..300 {
        let mut f = file(8000, 1, 16, &[]);
        for _ in 0..len {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            f.push(x as u8);
        }
        if len % 3 == 0 {
            f.truncate(12 + len % 40);
        }
        if let Ok(w) = parse(&f) {
            let mut out = vec![0i16; 64];
            let _ = w.decode_s16(0, &mut out);
            let _ = w.decode_s16(usize::MAX / 2, &mut out);
        }
    }
}
