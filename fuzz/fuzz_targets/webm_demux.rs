//! Fuzz target: the WebM demuxer (`kitsune_core::webm`) on attacker-controlled bytes (a downloaded
//! video, DASH segments from any server).
//!
//! Input layout: `[mode, chunk, seek_at, ...bytes]`.
//!
//! * `mode & 1 == 0`: `bytes` as they are (raw: EBML header parsing, `NotWebm`).
//! * `mode & 1 == 1`: `bytes` after a valid EBML header and an unknown-size Segment, so the
//!   fuzzer starts inside the segment (Info, Tracks, Cues, clusters, blocks, block groups).
//! * `mode & 2`: the segment header is followed by a valid `Tracks` (VP9 + Opus), so blocks get
//!   a track list.
//!
//! The bytes are pushed in pieces of `chunk + 1` bytes with `poll` drained after each; when
//! `seek_at` is reached the demuxer `seek`s to an offset taken from the cues (or to that byte) and
//! the rest is fed from there. Invariants: never a panic, every packet's data fits in
//! `webm::MAX_BLOCK`, at most `webm::MAX_TRACKS` tracks and `webm::MAX_CUES` cue points, the
//! cues are sorted, and pushing the same bytes in one piece or in small pieces gives the same
//! events up to the first error.
#![no_main]

use kitsune_core::webm::{self, Demuxer, Event};
use libfuzzer_sys::fuzz_target;

fn el(id: &[u8], body: &[u8]) -> Vec<u8> {
    let mut v = id.to_vec();
    v.push(0x01);
    v.extend_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
    v.extend_from_slice(body);
    v
}

fn prefix(mode: u8) -> Vec<u8> {
    let mut v = el(&[0x1A, 0x45, 0xDF, 0xA3], &el(&[0x42, 0x82], b"webm"));
    v.extend_from_slice(&[0x18, 0x53, 0x80, 0x67, 0xFF]);
    if mode & 2 != 0 {
        let video = [
            el(&[0xD7], &[1]),
            el(&[0x83], &[1]),
            el(&[0x86], b"V_VP9"),
            el(
                &[0xE0],
                &[el(&[0xB0], &[1, 0x40]), el(&[0xBA], &[0xB4])].concat(),
            ),
        ]
        .concat();
        let audio = [
            el(&[0xD7], &[2]),
            el(&[0x83], &[2]),
            el(&[0x86], b"A_OPUS"),
            el(&[0x63, 0xA2], b"OpusHead\x01\x02\x38\x01\x80\xbb\0\0\0\0\0"),
        ]
        .concat();
        v.extend_from_slice(&el(
            &[0x16, 0x54, 0xAE, 0x6B],
            &[el(&[0xAE], &video), el(&[0xAE], &audio)].concat(),
        ));
    }
    v
}

/// Push `data` in `chunk`-sized pieces, collecting events until the first error.
fn run(data: &[u8], chunk: usize, seek_at: Option<usize>) -> (Vec<Event>, Option<webm::Error>) {
    let mut d = Demuxer::new();
    let mut events = Vec::new();
    let mut at = 0;
    while at < data.len() {
        if seek_at == Some(at) && at > 0 {
            // Seek to what the cues say (if any) or to this very byte, and go on from there.
            let off = d.cluster_for(0).unwrap_or(at as u64);
            d.seek(off);
            at = usize::try_from(off).unwrap_or(usize::MAX).min(data.len());
            if at >= data.len() {
                break;
            }
        }
        let mut end = (at + chunk).min(data.len());
        if let Some(s) = seek_at
            && s > at
            && s < end
        {
            end = s;
        }
        if d.push(&data[at..end]).is_err() {
            break;
        }
        at = end;
        loop {
            match d.poll() {
                Ok(Some(e)) => {
                    check(&d, &e);
                    events.push(e);
                }
                Ok(None) => break,
                Err(e) => return (events, Some(e)),
            }
        }
    }
    (events, None)
}

fn check(d: &Demuxer, e: &Event) {
    if let Event::Packet(p) = e {
        assert!(p.data.len() as u64 <= webm::MAX_BLOCK);
    }
    assert!(d.tracks().len() <= webm::MAX_TRACKS);
    assert!(d.cues().len() <= webm::MAX_CUES);
    assert!(d.cues().windows(2).all(|w| w[0].time_ns <= w[1].time_ns));
    for t in d.tracks() {
        assert!(t.number > 0);
        let _ = webm::OpusHead::parse(&t.codec_private);
    }
}

fuzz_target!(|input: &[u8]| {
    if input.len() < 3 {
        return;
    }
    let (mode, chunk, seek) = (input[0], usize::from(input[1]) + 1, input[2]);
    let mut data = if mode & 1 != 0 {
        prefix(mode)
    } else {
        Vec::new()
    };
    data.extend_from_slice(&input[3..]);
    // One piece versus small pieces: the same events up to the first error.
    let (whole, whole_err) = run(&data, data.len().max(1), None);
    let (small, small_err) = run(&data, chunk, None);
    assert_eq!(whole_err, small_err);
    assert_eq!(whole, small);
    // With a seek in the middle: only "no panic, invariants hold".
    let seek_at = (data.len() * usize::from(seek)) / 256;
    let _ = run(&data, chunk, Some(seek_at));
});
