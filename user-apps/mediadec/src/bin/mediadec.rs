//! `mediadec`: decodes WebM (VP9 video + Opus audio, what YouTube serves) in a ring 3 process.
//!
//! ```text
//! run mediadec FILE.webm [--info] [--frames N] [--md5] [--pcm OUT.raw] [--rgb WxH] [--show]
//! run mediadec --serve-test FILE.webm   the message server driven in memory (protocol check)
//! mediadec --serve                      the message server of a player (started by the kernel)
//! ```
//!
//! The file mode prints the duration, the tracks, the resolution and how fast it decoded; `--md5`
//! prints the MD5 of every picture (the planes packed, as `ffmpeg -f framemd5`) and of the whole PCM,
//! `--pcm` writes the audio (s16le, stereo, 48 kHz), `--rgb` also converts each picture to RGBA at
//! that size (timed), `--show` plays the video in a window. The decoders are C (libvpx, libopus in
//! fixed point) compiled by build.rs; the demuxer is `kitsune_core::webm`. See
//! docs/design/midia.md.
#![no_std]
#![no_main]

extern crate alloc;

#[path = "mediadec/decode.rs"]
mod decode;
#[path = "mediadec/ffi.rs"]
mod ffi;
#[path = "mediadec/heap.rs"]
mod heap;
#[path = "mediadec/pipeline.rs"]
mod pipeline;
#[path = "mediadec/serve.rs"]
mod serve;
#[path = "mediadec/sys.rs"]
mod sys;

use alloc::vec::Vec;
use kitsune_user::fs::Env;
use kitsune_user::*;
use kitsune_userabi::errno;

entry!(main);

const USAGE: &str = "usage: mediadec FILE.webm [--info] [--frames N] [--md5] [--pcm OUT.raw] \
                     [--rgb WxH] [--show] | mediadec --serve-test FILE.webm\n";

fn main() -> i32 {
    let env = Env::load();
    let args: Vec<&str> = env.args().collect();
    match args.first().copied() {
        Some("--serve") => {
            serve::serve(&mut PageLink);
            0
        }
        Some("--serve-test") => match args.get(1) {
            Some(path) => serve_test(path),
            None => {
                print!("{USAGE}");
                2
            }
        },
        Some("-h" | "--help") | None => {
            print!("{USAGE}");
            if args.is_empty() { 2 } else { 0 }
        }
        Some(_) => match decode::Opts::parse(&args) {
            Some(o) => decode::run(&o),
            None => {
                print!("{USAGE}");
                2
            }
        },
    }
}

/// The browser's side of calls 56/57.
struct PageLink;

impl serve::Link for PageLink {
    fn recv(&mut self, buf: &mut [u8], timeout_ms: u64) -> Result<Option<usize>, ()> {
        loop {
            match page_recv(buf, timeout_ms) {
                n if n > 0 => return Ok(Some(n as usize)),
                // Nothing in time.
                0 => return Ok(None),
                // A message too large was dropped: look again.
                errno::E2BIG => continue,
                _ => return Err(()),
            }
        }
    }

    fn send(&mut self, msg: &[u8]) -> serve::Sent {
        match page_send(msg) {
            0 => serve::Sent::Done,
            // The player has not taken the earlier messages yet.
            errno::EAGAIN => serve::Sent::Full,
            _ => serve::Sent::Gone,
        }
    }
}

/// `--serve-test`: run the server in memory over a file and check what it sent.
fn serve_test(path: &str) -> i32 {
    let data = match decode::read_file(path) {
        Ok(d) => d,
        Err(e) => {
            println!("mediadec: {path}: {e}");
            return 2;
        }
    };
    let t0 = uptime_ms();
    let r = serve::selftest(&data);
    let ms = uptime_ms() - t0;
    println!(
        "serve-test: header {}, {} pictures {}x{}, {} samples per channel, drained says {}, buffered to {} ms in {} range(s), errors {}",
        r.header,
        r.pictures,
        r.picture_size.0,
        r.picture_size.1,
        r.samples,
        r.end_pictures,
        r.buffered_end_ms,
        r.buffered_ranges,
        r.errors
    );
    println!(
        "serve-test: seek to 1 s -> mark first {}, {} pictures after it, the first at {} ms; budget of 2 pictures held {}, {} after an ack ({ms} ms)",
        r.mark_first,
        r.after_seek_pictures,
        r.first_after_seek_ns / 1_000_000,
        r.budget_held,
        r.budget_after_ack
    );
    let ok = r.ok();
    println!("serve-test: {}", if ok { "PASS" } else { "FAIL" });
    if ok { 0 } else { 1 }
}
