//! The file mode: decode a WebM file from the file system, print what it holds, how fast it went
//! and (with `--md5`) a digest of every picture and of the audio.

use super::ffi::Picture;
use super::pipeline::{Fatal, Media, Out};
use alloc::vec;
use alloc::vec::Vec;
use kitsune_core::md5::{self, Md5};
use kitsune_user::fs::{self, File};
use kitsune_user::win::{self, Window};
use kitsune_user::*;
use kitsune_userabi::EV_CLOSE;

/// The largest file read at once by `--serve-test` (the file mode streams).
const MAX_FILE: usize = 64 * 1024 * 1024;

/// The command line of the file mode.
pub struct Opts<'a> {
    pub path: &'a str,
    pub info: bool,
    pub frames: Option<u32>,
    pub md5: bool,
    pub pcm: Option<&'a str>,
    pub rgb: Option<(u32, u32)>,
    pub show: bool,
}

impl<'a> Opts<'a> {
    pub fn parse(args: &[&'a str]) -> Option<Opts<'a>> {
        let mut o = Opts {
            path: "",
            info: false,
            frames: None,
            md5: false,
            pcm: None,
            rgb: None,
            show: false,
        };
        let mut it = args.iter();
        while let Some(&a) = it.next() {
            match a {
                "--info" => o.info = true,
                "--md5" => o.md5 = true,
                "--show" => o.show = true,
                "--frames" => o.frames = Some(it.next()?.parse().ok()?),
                "--pcm" => o.pcm = Some(it.next()?),
                "--rgb" => {
                    let (w, h) = it.next()?.split_once('x')?;
                    let (w, h) = (w.parse().ok()?, h.parse().ok()?);
                    if w == 0 || h == 0 || w > 4096 || h > 4096 {
                        return None;
                    }
                    o.rgb = Some((w, h));
                }
                _ if a.starts_with("--") || !o.path.is_empty() => return None,
                _ => o.path = a,
            }
        }
        (!o.path.is_empty()).then_some(o)
    }
}

/// Read a whole file (at most [`MAX_FILE`]).
pub fn read_file(path: &str) -> Result<Vec<u8>, &'static str> {
    let mut f = File::open(path).map_err(fs::error_text)?;
    let mut out = Vec::new();
    loop {
        let at = out.len();
        if at >= MAX_FILE {
            return Err("file too large (64 MiB at most)");
        }
        out.resize(at + 64 * 1024, 0);
        let n = f.read(&mut out[at..]).map_err(fs::error_text)?;
        out.truncate(at + n);
        if n == 0 {
            return Ok(out);
        }
    }
}

/// Counters of a run.
#[derive(Default)]
struct Tally {
    pictures: u32,
    video_ms: u64,
    rgb_ms: u64,
    samples: u64,
    audio_packets: u32,
    audio_ms: u64,
    errors: u32,
}

pub fn run(o: &Opts) -> i32 {
    let mut file = match File::open(o.path) {
        Ok(f) => f,
        Err(e) => {
            println!("mediadec: {}: {}", o.path, fs::error_text(e));
            return 2;
        }
    };
    let mut pcm_out = match o.pcm.map(File::create) {
        Some(Err(e)) => {
            println!("mediadec: {}: {}", o.pcm.unwrap_or(""), fs::error_text(e));
            return 2;
        }
        Some(Ok(f)) => Some(f),
        None => None,
    };
    let mut media = Media::new();
    let mut t = Tally::default();
    let mut audio_md5 = Md5::new();
    let mut rgba: Vec<u32> = Vec::new();
    let mut window: Option<Window> = None;
    let mut buf = vec![0u8; 64 * 1024];
    let started = uptime_ms();
    let mut play_start: Option<(u64, i64)> = None;
    'file: loop {
        let n = match file.read(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                println!("mediadec: {}: {}", o.path, fs::error_text(e));
                return 2;
            }
        };
        if n == 0 {
            break;
        }
        if let Err(e) = media.push(&buf[..n]) {
            return fatal(e);
        }
        loop {
            let t0 = uptime_ms();
            let out = match media.poll() {
                Ok(Some(out)) => out,
                Ok(None) => break,
                Err(e) => return fatal(e),
            };
            let spent = uptime_ms() - t0;
            match out {
                Out::Header => {
                    header(&media, o.path);
                    if o.info {
                        return 0;
                    }
                }
                Out::Picture { ts_ns, picture } => {
                    t.video_ms += spent;
                    t.pictures += 1;
                    if o.md5 {
                        let mut h = Md5::new();
                        picture.for_each_row(|r| h.update(r));
                        let hex = md5::hex(&h.finish());
                        println!(
                            "frame {} {} ms {}",
                            t.pictures - 1,
                            ts_ns / 1_000_000,
                            core::str::from_utf8(&hex).unwrap_or("")
                        );
                    }
                    if let Some((w, h)) = o.rgb {
                        let t1 = uptime_ms();
                        convert(&picture, &mut rgba, w, h);
                        t.rgb_ms += uptime_ms() - t1;
                    }
                    if o.show && !show(&picture, &mut rgba, &mut window, &mut play_start, ts_ns) {
                        break 'file;
                    }
                    if o.frames.is_some_and(|n| t.pictures >= n) {
                        break 'file;
                    }
                }
                Out::Audio { pcm, .. } => {
                    t.audio_ms += spent;
                    t.audio_packets += 1;
                    t.samples += (pcm.len() / 2) as u64;
                    if o.md5 || pcm_out.is_some() {
                        let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
                        audio_md5.update(&bytes);
                        if let Some(f) = pcm_out.as_mut()
                            && let Err(e) = f.write_all(&bytes)
                        {
                            println!("mediadec: {}: {}", o.pcm.unwrap_or(""), fs::error_text(e));
                            return 2;
                        }
                    }
                }
                Out::DecodeError { audio, code } => {
                    t.errors += 1;
                    if audio {
                        println!("mediadec: audio packet did not decode (error {code})");
                    } else {
                        let why = media.video_error();
                        println!("mediadec: video packet did not decode (error {code}: {why})");
                    }
                }
            }
        }
    }
    let total = uptime_ms() - started;
    report(&media, &t, total, o.rgb);
    if o.md5 {
        let hex = md5::hex(&audio_md5.finish());
        println!(
            "audio md5 {} ({} samples per channel)",
            core::str::from_utf8(&hex).unwrap_or(""),
            t.samples
        );
    }
    i32::from(t.errors > 0)
}

fn fatal(e: Fatal) -> i32 {
    match e {
        Fatal::Demux(d) => println!("mediadec: {d}"),
        Fatal::NoDecoder => println!("mediadec: cannot create a decoder (out of memory)"),
    }
    1
}

fn header(media: &Media, path: &str) {
    let s = media.summary();
    let d = s.duration_ns / 1_000_000;
    println!("mediadec: {path}: {}.{:03} s", d / 1000, d % 1000);
    for tr in media.demuxer().tracks() {
        println!(
            "  track {}: {:?} {} {}x{} {} Hz {} ch{}",
            tr.number,
            tr.kind,
            tr.codec_id,
            tr.width,
            tr.height,
            tr.sample_rate,
            tr.channels,
            if Some(tr.number) == s.video_track || Some(tr.number) == s.audio_track {
                ""
            } else {
                " (not decoded)"
            }
        );
    }
    if s.video_track.is_some() {
        println!("  video: VP9 {}x{}", s.width, s.height);
    }
    if s.audio_track.is_some() {
        println!(
            "  audio: Opus {} ch -> stereo 48 kHz, pre-skip {}",
            s.channels, s.pre_skip
        );
    }
}

/// `n` per `ms` milliseconds as "x.y" per second.
fn rate(n: u64, ms: u64) -> (u64, u64) {
    let tenths = n * 10_000 / ms.max(1);
    (tenths / 10, tenths % 10)
}

fn report(media: &Media, t: &Tally, total_ms: u64, rgb: Option<(u32, u32)>) {
    let s = media.summary();
    let (fps, fps10) = rate(u64::from(t.pictures), t.video_ms);
    println!(
        "video: {} pictures {}x{}, {} ms decoding: {fps}.{fps10} fps",
        t.pictures, s.width, s.height, t.video_ms
    );
    if let Some((w, h)) = rgb {
        let per = t.rgb_ms * 10 / u64::from(t.pictures.max(1));
        println!(
            "rgba {w}x{h}: {} ms, {}.{} ms per picture",
            t.rgb_ms,
            per / 10,
            per % 10
        );
    }
    let audio_ms = t.samples * 1000 / 48_000;
    println!(
        "audio: {} packets, {} samples per channel ({} ms of sound), {} ms decoding",
        t.audio_packets, t.samples, audio_ms, t.audio_ms
    );
    println!(
        "total: {total_ms} ms, {} errors, {} laced blocks skipped",
        t.errors,
        media.demuxer().skipped()
    );
}

fn convert(p: &Picture, rgba: &mut Vec<u32>, w: u32, h: u32) {
    let n = (w * h) as usize;
    if rgba.len() < n {
        rgba.resize(n, 0);
    }
    p.to_rgba(rgba, w, h);
}

/// Show `p` in the window (opened at the first picture), at its time. `false` when the window
/// was closed.
fn show(
    p: &Picture,
    rgba: &mut Vec<u32>,
    window: &mut Option<Window>,
    start: &mut Option<(u64, i64)>,
    ts_ns: i64,
) -> bool {
    let (w, h) = (p.width().min(1280), p.height().min(800));
    if window.is_none() {
        match Window::open(w, h, "mediadec") {
            Ok(win) => *window = Some(win),
            Err(_) => return false,
        }
    }
    convert(p, rgba, w, h);
    let bytes: Vec<u8> = rgba[..(w * h) as usize]
        .iter()
        .flat_map(|px| px.to_le_bytes())
        .collect();
    let (t0, ts0) = *start.get_or_insert((uptime_ms(), ts_ns));
    let due = t0 + ((ts_ns - ts0).max(0) / 1_000_000) as u64;
    let now = uptime_ms();
    if due > now {
        sleep_ms(due - now);
    }
    if let Some(win) = window.as_ref() {
        let _ = win.present(&bytes, w, h);
    }
    while let Ok(Some(ev)) = win::next_event(0) {
        if ev.kind == EV_CLOSE {
            return false;
        }
    }
    true
}
