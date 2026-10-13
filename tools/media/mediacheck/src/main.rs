//! `mediacheck FILE.webm [--vmd5 REF] [--aref REF.raw] [--pcm OUT.raw] [--chunk N] [--rgb WxH]`
//!
//! Decodes FILE with the code of `mediadec` (same C, same pipeline) on the host and checks it:
//! every picture's MD5 against `ffmpeg -f framemd5` (`--vmd5`), the PCM against ffmpeg's decode
//! (`--aref`, s16le stereo 48 kHz: exact sample count and SNR), and prints the decoding speed.
//! `--chunk` feeds the file in pieces of N bytes (default 65536), `--rgb` also converts every picture
//! to RGBA at that size (and times it). Exit status 0 when every check given passes.

extern crate alloc;

#[path = "../../../../user-apps/mediadec/src/bin/mediadec/ffi.rs"]
mod ffi;
#[path = "../../../../user-apps/mediadec/src/bin/mediadec/pipeline.rs"]
mod pipeline;
#[path = "../../../../user-apps/mediadec/src/bin/mediadec/serve.rs"]
#[allow(dead_code)]
mod serve;

use kitsune_core::md5;
use pipeline::{Media, Out};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut file = None;
    let (mut vmd5, mut aref, mut pcm_out, mut chunk, mut rgb) = (None, None, None, 65536, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--vmd5" => vmd5 = it.next().cloned(),
            "--aref" => aref = it.next().cloned(),
            "--pcm" => pcm_out = it.next().cloned(),
            "--chunk" => chunk = it.next().and_then(|s| s.parse().ok()).unwrap_or(65536),
            "--rgb" => {
                rgb = it.next().and_then(|s| {
                    let (w, h) = s.split_once('x')?;
                    Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))
                })
            }
            "--serve-test" => {}
            _ => file = Some(a.clone()),
        }
    }
    let file = file.expect("usage: mediacheck FILE.webm [--vmd5 REF] [--aref REF.raw] ...");
    let data = std::fs::read(&file).expect("read the file");
    if args.iter().any(|a| a == "--serve-test") {
        // The message server over an in-memory link, as `mediadec --serve-test` runs it.
        let r = serve::selftest(&data);
        println!("{r:?}");
        let ok = r.header
            && r.picture_size == (160, 90)
            && r.end_pictures == r.pictures
            && r.errors == 0
            && r.first_after_seek_ns == 1_000_000_000;
        println!("serve-test: {}", if ok { "PASS" } else { "FAIL" });
        std::process::exit(i32::from(!ok));
    }
    let want: Option<Vec<String>> = vmd5.map(|p| {
        std::fs::read_to_string(p)
            .expect("read the framemd5")
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| l.rsplit(',').next().map(|s| s.trim().to_string()))
            .collect()
    });

    let mut media = Media::new();
    let mut frames: Vec<String> = Vec::new();
    let mut pcm: Vec<i16> = Vec::new();
    let mut rgba: Vec<u32> = rgb.map_or(Vec::new(), |(w, h)| vec![0; (w * h) as usize]);
    let (mut t_video, mut t_audio, mut t_rgb) = (Duration::ZERO, Duration::ZERO, Duration::ZERO);
    let mut errors = 0;
    let started = Instant::now();
    'file: for piece in data.chunks(chunk.max(1)) {
        media.push(piece).expect("push");
        loop {
            let t0 = Instant::now();
            let out = match media.poll() {
                Ok(Some(o)) => o,
                Ok(None) => break,
                Err(e) => {
                    println!("stream error: {e:?}");
                    errors += 1;
                    break 'file;
                }
            };
            let spent = t0.elapsed();
            match out {
                Out::Header => {
                    let s = media.summary();
                    println!(
                        "header: {} ms, video track {:?} {}x{}, audio track {:?} {} ch, pre-skip {}",
                        s.duration_ns / 1_000_000,
                        s.video_track,
                        s.width,
                        s.height,
                        s.audio_track,
                        s.channels,
                        s.pre_skip
                    );
                }
                Out::Picture { picture, .. } => {
                    t_video += spent;
                    let mut h = md5::Md5::new();
                    picture.for_each_row(|r| h.update(r));
                    frames.push(String::from_utf8(md5::hex(&h.finish()).to_vec()).unwrap());
                    if let Some((w, hh)) = rgb {
                        let t1 = Instant::now();
                        picture.to_rgba(&mut rgba, w, hh);
                        t_rgb += t1.elapsed();
                    }
                }
                Out::Audio { pcm: p, .. } => {
                    t_audio += spent;
                    pcm.extend_from_slice(p);
                }
                Out::DecodeError { audio, code } => {
                    errors += 1;
                    println!("decode error (audio {audio}): {code}");
                }
            }
        }
    }
    let total = started.elapsed();
    let mut ok = errors == 0;
    let n = frames.len();
    println!(
        "video: {n} pictures in {:.1} ms ({:.0} fps)",
        t_video.as_secs_f64() * 1e3,
        n as f64 / t_video.as_secs_f64().max(1e-9)
    );
    if rgb.is_some() {
        println!(
            "rgba: {:.2} ms per picture",
            t_rgb.as_secs_f64() * 1e3 / n.max(1) as f64
        );
    }
    println!(
        "audio: {} samples per channel ({:.3} s) in {:.1} ms; total {:.1} ms",
        pcm.len() / 2,
        pcm.len() as f64 / 2.0 / 48_000.0,
        t_audio.as_secs_f64() * 1e3,
        total.as_secs_f64() * 1e3
    );
    if let Some(want) = want {
        let same = frames.iter().zip(&want).filter(|(a, b)| a == b).count();
        let first_bad = frames.iter().zip(&want).position(|(a, b)| a != b);
        let pass = same == want.len() && frames.len() == want.len();
        println!(
            "md5: {same}/{} pictures identical to ffmpeg (we decoded {}){}",
            want.len(),
            frames.len(),
            first_bad.map_or(String::new(), |i| format!(
                ", first difference at picture {i}"
            ))
        );
        ok &= pass;
    }
    if let Some(path) = aref {
        let raw = std::fs::read(path).expect("read the audio reference");
        let r: Vec<i16> = raw
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        let (mut sig, mut noise) = (0f64, 0f64);
        for (a, b) in r.iter().zip(&pcm) {
            sig += f64::from(*a) * f64::from(*a);
            let d = f64::from(*a) - f64::from(*b);
            noise += d * d;
        }
        let snr = if noise == 0.0 {
            f64::INFINITY
        } else {
            10.0 * (sig / noise).log10()
        };
        let pass = r.len() == pcm.len() && snr > 40.0;
        println!(
            "pcm: {} samples per channel, reference {}; SNR {:.1} dB",
            pcm.len() / 2,
            r.len() / 2,
            snr
        );
        ok &= pass;
    }
    if let Some(path) = pcm_out {
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        std::fs::write(path, bytes).expect("write the PCM");
    }
    println!("{}", if ok { "PASS" } else { "FAIL" });
    std::process::exit(if ok { 0 } else { 1 });
}
