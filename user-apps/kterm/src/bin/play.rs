//! `play`: sound from the Terminal (the shell's `play`, `tone`, `beep` and `volume` run it).
//!
//! ```text
//! play ARQUIVO.wav [-v VOL] [--clock]      a WAV file (PCM 8/16/24/32 bits or float, mono/stereo)
//! play --tone FREQ[,FREQ...] MS [-v VOL]    sine tones; several frequencies = one stream each (a chord)
//! play --volume [N|mute|unmute]            show or set the system volume
//! play --gen ARQ.wav FREQ MS [RATE [CH [BITS]]]   write a WAV with a tone (for tests)
//! ```
//!
//! It lives in the `kterm` crate because both use `kitsune_core` (the WAV parser and the tone
//! generator), which is then compiled once for the two. The samples go to the kernel mixer through a
//! stream (`kitsune_user::audio`); the program waits until the last sample was heard before it
//! exits, and `--clock` prints the stream's clock (frames the device played) next to the uptime
//! every 250 ms, which is what a video player follows.
#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use kitsune_core::hw::audio::Tone;
use kitsune_core::i18n::{self, Lang};
use kitsune_core::t;
use kitsune_core::wav;
use kitsune_user::audio::{self, Stream};
use kitsune_user::fs::{self, Env, File};
use kitsune_user::*;

entry!(main);

/// Peak of a tone (about -6 dBFS; a chord shares it).
const AMP: i32 = 16_000;
/// Samples per write.
const CHUNK: usize = 4096;

fn main() -> i32 {
    i18n::set_lang(Lang::from_index(language()));
    let env = Env::load();
    let args: Vec<&str> = env.args().collect();
    let mut volume: Option<u8> = None;
    let mut clock = false;
    let mut rest: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-v" | "--vol" => {
                i += 1;
                volume = args
                    .get(i)
                    .and_then(|v| v.parse().ok())
                    .filter(|v| *v <= 100);
                if volume.is_none() {
                    println!("{}", t!("play.usage"));
                    return 2;
                }
            }
            "--clock" => clock = true,
            a => rest.push(a),
        }
        i += 1;
    }
    let code = match rest.as_slice() {
        ["--volume"] => show_volume(),
        ["--volume", v] => set_volume(v),
        ["--tone", freqs, ms] => tone(freqs, ms, volume, clock),
        ["--gen", out, f, ms, more @ ..] => generate(out, f, ms, more),
        [file] if !file.starts_with('-') => play_file(file, volume, clock),
        _ => {
            println!("{}", t!("play.usage"));
            2
        }
    };
    code
}

fn err_text(e: i64) -> String {
    match e {
        abi::errno::ENODEV => String::from(t!("play.no_device")),
        abi::errno::EMFILE => String::from(t!("play.too_many")),
        e => String::from(fs::error_text(e)),
    }
}

fn show_volume() -> i32 {
    let (v, m) = (audio::master(), audio::muted());
    if v < 0 {
        println!("play: {}", err_text(v));
        return 1;
    }
    println!(
        "{}",
        t!(
            "play.volume",
            volume = v as u64,
            muted = if m == 1 { t!("play.muted") } else { "" }
        )
    );
    0
}

fn set_volume(v: &str) -> i32 {
    let r = match v {
        "mute" => audio::set_muted(true),
        "unmute" => audio::set_muted(false),
        n => match n.trim_end_matches('%').parse::<u8>() {
            Ok(n) if n <= 100 => audio::set_master(n),
            _ => {
                println!("{}", t!("play.usage"));
                return 2;
            }
        },
    };
    if r < 0 {
        println!("play: {}", err_text(r));
        return 1;
    }
    show_volume()
}

/// Little-endian bytes of samples.
fn le_bytes(samples: &[i16], out: &mut Vec<u8>) {
    out.clear();
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
}

/// Print the clock line every 250 ms when asked.
struct Clock {
    on: bool,
    next: u64,
}

impl Clock {
    fn tick(&mut self, s: &Stream) {
        if !self.on || uptime_ms() < self.next {
            return;
        }
        self.next = uptime_ms() + 250;
        if let Ok(st) = s.status() {
            println!(
                "clock uptime_ms={} played_ms={} queued={} underruns={}",
                uptime_ms(),
                st.played_ms(),
                st.queued_bytes,
                st.underruns
            );
        }
    }
}

/// Wait for the end of the stream (printing the clock), then the summary line.
fn finish(s: &Stream, clock: &mut Clock) -> i32 {
    while !s.drain(250) {
        clock.tick(s);
        if s.status().is_err() {
            break;
        }
    }
    clock.tick(s);
    match s.status() {
        Ok(st) => {
            println!(
                "{}",
                t!(
                    "play.done",
                    frames = st.played,
                    ms = st.played_ms(),
                    underruns = u64::from(st.underruns)
                )
            );
            0
        }
        Err(e) => {
            println!("play: {}", err_text(e));
            1
        }
    }
}

fn open_stream(rate: u32, channels: u32, volume: Option<u8>) -> Result<Stream, i32> {
    match Stream::open(rate, channels) {
        Ok(s) => {
            if let Some(v) = volume {
                s.set_volume(v);
            }
            Ok(s)
        }
        Err(e) => {
            println!("play: {}", err_text(e));
            Err(1)
        }
    }
}

fn tone(freqs: &str, ms: &str, volume: Option<u8>, clock: bool) -> i32 {
    let freqs: Vec<u32> = freqs.split(',').filter_map(|f| f.parse().ok()).collect();
    let Ok(ms) = ms.parse::<u32>() else {
        println!("{}", t!("play.usage"));
        return 2;
    };
    if freqs.is_empty() || freqs.len() > 4 || freqs.iter().any(|f| !(20..=20_000).contains(f)) {
        println!("{}", t!("play.usage"));
        return 2;
    }
    let ms = ms.min(60_000);
    let amp = (AMP / freqs.len() as i32) as i16;
    let mut tones: Vec<Tone> = freqs
        .iter()
        .map(|&f| Tone::new(f, 48_000, ms, amp))
        .collect();
    let mut streams = Vec::new();
    for _ in &freqs {
        match open_stream(48_000, 1, volume) {
            Ok(s) => streams.push(s),
            Err(c) => return c,
        }
    }
    let list: Vec<String> = freqs.iter().map(|f| alloc::format!("{f}")).collect();
    println!(
        "{}",
        t!(
            "play.tone",
            freqs = list.join(" + ").as_str(),
            ms = u64::from(ms)
        )
    );
    // Several streams start together: paused while the first blocks go in.
    let chord = streams.len() > 1;
    if chord {
        for s in &streams {
            s.set_paused(true);
        }
    }
    let mut clock = Clock { on: clock, next: 0 };
    let mut buf = vec![0i16; CHUNK];
    let mut bytes = Vec::with_capacity(CHUNK * 2);
    let mut started = !chord;
    loop {
        let mut any = false;
        for (t, s) in tones.iter_mut().zip(&streams) {
            let n = t.fill(&mut buf);
            if n == 0 {
                continue;
            }
            any = true;
            le_bytes(&buf[..n], &mut bytes);
            if !started
                && s.status()
                    .is_ok_and(|st| st.free_bytes < bytes.len() as u32)
            {
                // The queues are full: let them all go at once.
                for s in &streams {
                    s.set_paused(false);
                }
                started = true;
            }
            if let Err(e) = s.write_all(&bytes) {
                println!("play: {}", err_text(e));
                return 1;
            }
        }
        clock.tick(&streams[0]);
        if !any {
            break;
        }
    }
    if !started {
        for s in &streams {
            s.set_paused(false);
        }
    }
    let mut code = 0;
    for s in &streams {
        code = finish(s, &mut clock);
    }
    code
}

/// Read up to `buf.len()` bytes (fewer only at the end of the file).
fn read_full(f: &mut File, buf: &mut [u8]) -> Result<usize, i64> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

fn play_file(path: &str, volume: Option<u8>, clock: bool) -> i32 {
    let size = match fs::stat(path) {
        Ok(st) => st.size as usize,
        Err(e) => {
            println!("play: {path}: {}", fs::error_text(e));
            return 1;
        }
    };
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            println!("play: {path}: {}", fs::error_text(e));
            return 1;
        }
    };
    // The header and anything before the samples must be in the first MiB.
    let mut head = vec![0u8; size.min(1 << 20)];
    let got = match read_full(&mut f, &mut head) {
        Ok(n) => n,
        Err(e) => {
            println!("play: {path}: {}", fs::error_text(e));
            return 1;
        }
    };
    let w = match wav::parse(&head[..got]) {
        Ok(w) => w,
        Err(e) => {
            println!(
                "{}",
                t!(
                    "play.bad_wav",
                    file = path,
                    why = alloc::format!("{e:?}").as_str()
                )
            );
            return 1;
        }
    };
    let (rate, channels, format) = (w.rate, w.channels, w.format);
    let fb = w.frame_bytes();
    let off = w.data.as_ptr() as usize - head.as_ptr() as usize;
    // The data chunk's own size (the 4 bytes before it), cut to what the file holds.
    let declared =
        u32::from_le_bytes([head[off - 4], head[off - 3], head[off - 2], head[off - 1]]) as usize;
    let total_frames = declared.min(size.saturating_sub(off)) / fb.max(1);
    let bits = match format {
        wav::SampleFormat::Int(b) => u64::from(b),
        wav::SampleFormat::Float32 => 32,
    };
    println!(
        "{}",
        t!(
            "play.file",
            file = path,
            rate = u64::from(rate),
            channels = u64::from(channels),
            bits = bits,
            ms = total_frames as u64 * 1000 / u64::from(rate)
        )
    );
    if !(abi::MIN_RATE..=abi::MAX_RATE).contains(&rate) {
        println!("{}", t!("play.bad_rate", rate = u64::from(rate)));
        return 1;
    }
    let s = match open_stream(rate, u32::from(channels), volume) {
        Ok(s) => s,
        Err(c) => return c,
    };
    if f.seek(off as i64, abi::SEEK_SET).is_err() {
        return 1;
    }
    let mut clock = Clock { on: clock, next: 0 };
    let chunk_frames = (32 * 1024 / fb).max(1);
    let mut raw = vec![0u8; chunk_frames * fb];
    let mut pcm = vec![0i16; chunk_frames * usize::from(channels)];
    let mut bytes = Vec::with_capacity(pcm.len() * 2);
    let mut done = 0usize;
    while done < total_frames {
        let want = (total_frames - done).min(chunk_frames) * fb;
        let n = match read_full(&mut f, &mut raw[..want]) {
            Ok(n) => n - n % fb,
            Err(_) => 0,
        };
        if n == 0 {
            break;
        }
        let part = wav::Wav {
            rate,
            channels,
            format,
            data: &raw[..n],
        };
        let frames = part.decode_s16(0, &mut pcm);
        le_bytes(&pcm[..frames * usize::from(channels)], &mut bytes);
        if let Err(e) = s.write_all(&bytes) {
            println!("play: {}", err_text(e));
            return 1;
        }
        done += frames;
        clock.tick(&s);
    }
    finish(&s, &mut clock)
}

/// `--gen OUT FREQ MS [RATE [CH [BITS]]]`: a WAV with a tone (8 or 16 bits, mono or stereo).
fn generate(out: &str, f: &str, ms: &str, more: &[&str]) -> i32 {
    let num = |s: Option<&&str>, d: u32| s.map_or(Some(d), |s| s.parse().ok());
    let (Ok(f), Ok(ms), Some(rate), Some(ch), Some(bits)) = (
        f.parse::<u32>(),
        ms.parse::<u32>(),
        num(more.first(), 48_000),
        num(more.get(1), 1),
        num(more.get(2), 16),
    ) else {
        println!("{}", t!("play.usage"));
        return 2;
    };
    if !(1..=2).contains(&ch) || !(bits == 8 || bits == 16) || !(1000..=192_000).contains(&rate) {
        println!("{}", t!("play.usage"));
        return 2;
    }
    let mut tone = Tone::new(f, rate, ms.min(60_000), AMP as i16);
    let frames = tone.remaining() as usize;
    let data_len = (frames * ch as usize * bits as usize / 8) as u32;
    let mut file = match File::create(out) {
        Ok(file) => file,
        Err(e) => {
            println!("play: {out}: {}", fs::error_text(e));
            return 1;
        }
    };
    let mut bytes = Vec::from(wav::header(rate, ch as u16, bits as u16, data_len));
    let mut buf = vec![0i16; CHUNK];
    loop {
        let n = tone.fill(&mut buf);
        if n == 0 {
            break;
        }
        for &s in &buf[..n] {
            for _ in 0..ch {
                if bits == 8 {
                    bytes.push(((s >> 8) + 128) as u8);
                } else {
                    bytes.extend_from_slice(&s.to_le_bytes());
                }
            }
        }
        if file.write_all(&bytes).is_err() {
            println!("play: {out}: {}", fs::error_text(abi::errno::EIO));
            return 1;
        }
        bytes.clear();
    }
    println!(
        "{}",
        t!("play.generated", file = out, frames = frames as u64)
    );
    0
}
