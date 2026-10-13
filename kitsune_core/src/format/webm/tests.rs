use super::ebml;
use super::*;
use std::vec;
use std::vec::Vec;

/// 320x180 VP9 + Opus, 1 s, made by `tools/media/gen-test-media.sh` (ffmpeg, libvpx-vp9, libopus).
const SMALL: &[u8] = include_bytes!("testdata/small.webm");
/// `ffprobe` of it: stream index, pts in ms (audio shifted by the codec delay), key flag, size.
const SMALL_PACKETS: &str = include_str!("testdata/small.packets.txt");

/// Push `data` in pieces of the sizes `chunks` gives (cycled) and collect every event.
fn run(data: &[u8], chunks: &[usize]) -> Result<(Demuxer, Vec<Event>), Error> {
    let mut d = Demuxer::new();
    let mut events = Vec::new();
    let mut at = 0;
    let mut i = 0;
    while at < data.len() {
        let n = chunks[i % chunks.len()].max(1).min(data.len() - at);
        i += 1;
        d.push(&data[at..at + n])?;
        at += n;
        while let Some(e) = d.poll()? {
            events.push(e);
        }
    }
    Ok((d, events))
}

fn packets(events: &[Event]) -> Vec<&Packet> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Packet(p) => Some(p),
            Event::Header => None,
        })
        .collect()
}

// ---- EBML primitives

#[test]
fn vint_lengths_and_unknown_size() {
    assert_eq!(ebml::vint(&[0x81]).unwrap(), Some((1, 1)));
    assert_eq!(ebml::vint(&[0x40, 0x02]).unwrap(), Some((2, 2)));
    assert_eq!(ebml::vint(&[0x40]).unwrap(), None);
    assert_eq!(ebml::vint(&[]).unwrap(), None);
    assert!(ebml::vint(&[0x00, 1, 2, 3, 4, 5, 6, 7, 8]).is_err());
    let h = ebml::header(&[
        0x1A, 0x45, 0xDF, 0xA3, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    ])
    .unwrap()
    .unwrap();
    assert_eq!((h.id, h.size, h.len), (0x1A45_DFA3, ebml::UNKNOWN, 12));
    let h = ebml::header(&[0xA3, 0xFF]).unwrap().unwrap();
    assert_eq!(h.size, ebml::UNKNOWN);
    assert!(ebml::header(&[0x08, 0, 0, 0, 0x81]).is_err(), "5-byte id");
}

#[test]
fn typed_values() {
    assert_eq!(ebml::uint(&[]).unwrap(), 0);
    assert_eq!(ebml::uint(&[1, 0]).unwrap(), 256);
    assert!(ebml::uint(&[0; 9]).is_err());
    assert_eq!(ebml::int(&[0xFF]).unwrap(), -1);
    assert_eq!(ebml::int(&[0xFF, 0x38]).unwrap(), -200);
    assert_eq!(ebml::int(&[0x7F]).unwrap(), 127);
    assert_eq!(ebml::float(&1.5f32.to_be_bytes()).unwrap(), 1.5);
    assert_eq!(ebml::float(&2.25f64.to_be_bytes()).unwrap(), 2.25);
    assert!(ebml::float(&[0; 3]).is_err());
    assert_eq!(ebml::string(b"webm\0\0"), b"webm");
}

// ---- the real file

#[test]
fn small_file_header() {
    let (d, events) = run(SMALL, &[SMALL.len()]).unwrap();
    assert_eq!(events.first(), Some(&Event::Header));
    assert_eq!(d.info().timescale_ns, 1_000_000);
    let dur = d.info().duration_ns.unwrap();
    assert!((990_000_000..=1_010_000_000).contains(&dur), "{dur}");
    let v = d
        .tracks()
        .iter()
        .find(|t| t.kind == TrackKind::Video)
        .unwrap();
    assert_eq!((v.codec, v.codec_id.as_str()), (Codec::Vp9, "V_VP9"));
    assert_eq!((v.width, v.height), (320, 180));
    assert!(!v.encoded);
    let a = d
        .tracks()
        .iter()
        .find(|t| t.kind == TrackKind::Audio)
        .unwrap();
    assert_eq!(
        (a.codec, a.sample_rate, a.channels),
        (Codec::Opus, 48_000, 1)
    );
    let head = OpusHead::parse(&a.codec_private).unwrap();
    assert_eq!(
        (head.channels, head.pre_skip, head.mapping_family),
        (1, 312, 0)
    );
    // Pre-skip in nanoseconds: 312 samples at 48 kHz.
    assert_eq!(a.codec_delay_ns, 6_500_000);
    assert_eq!(a.seek_preroll_ns, 80_000_000);
    assert!(!d.cues().is_empty(), "ffmpeg writes cues");
}

#[test]
fn small_file_packets_match_ffprobe() {
    let (d, events) = run(SMALL, &[SMALL.len()]).unwrap();
    let got = packets(&events);
    let want: Vec<(u64, i64, bool, usize)> = SMALL_PACKETS
        .lines()
        .map(|l| {
            let f: Vec<i64> = l.split(' ').map(|x| x.parse().unwrap()).collect();
            (f[0] as u64, f[1], f[2] == 1, f[3] as usize)
        })
        .collect();
    assert_eq!(got.len(), want.len());
    let video = d
        .tracks()
        .iter()
        .find(|t| t.kind == TrackKind::Video)
        .unwrap()
        .number;
    let audio = d
        .tracks()
        .iter()
        .find(|t| t.kind == TrackKind::Audio)
        .unwrap();
    for (p, &(stream, ms, key, size)) in got.iter().zip(&want) {
        let (number, shift) = if stream == 0 {
            (video, 0)
        } else {
            (audio.number, audio.codec_delay_ns as i64)
        };
        assert_eq!(p.track, number);
        assert_eq!(p.data.len(), size);
        assert_eq!(p.keyframe, key, "packet at {ms} ms");
        // ffprobe prints milliseconds rounded, with the codec delay taken off audio.
        let diff = (p.timestamp_ns - shift) - ms * 1_000_000;
        assert!(diff.abs() <= 500_000, "{} vs {ms} ms", p.timestamp_ns);
    }
    // The last audio packet carries the end trimming (DiscardPadding).
    let last_audio = got.iter().rev().find(|p| p.track == audio.number).unwrap();
    assert!(last_audio.discard_padding_ns > 0);
    assert_eq!(d.skipped(), 0);
}

#[test]
fn any_chunking_gives_the_same_packets() {
    let (_, whole) = run(SMALL, &[SMALL.len()]).unwrap();
    for chunks in [
        &[1usize][..],
        &[2, 3, 5, 7],
        &[4096],
        &[1000, 1, 17, 30_000],
    ] {
        let (_, ev) = run(SMALL, chunks).unwrap();
        assert_eq!(ev, whole, "chunks {chunks:?}");
    }
}

#[test]
fn seek_to_a_cue_restarts_at_a_keyframe() {
    let (d0, all) = run(SMALL, &[SMALL.len()]).unwrap();
    let cues = d0.cues().to_vec();
    assert!(cues.len() >= 2, "{cues:?}");
    let target = cues[1];
    let mut d = Demuxer::new();
    // Only the head of the file (up to the first cluster), as a DASH init segment.
    let first_cluster = usize::try_from(cues[0].cluster_pos).unwrap();
    d.push(&SMALL[..first_cluster]).unwrap();
    assert_eq!(d.poll().unwrap(), Some(Event::Header));
    assert_eq!(d.poll().unwrap(), None);
    // Without the cues (they are at the end of this file), ask the full demuxer where to go.
    assert_eq!(d0.cluster_for(target.time_ns), Some(target.cluster_pos));
    assert_eq!(d0.cluster_for(target.time_ns + 1), Some(target.cluster_pos));
    assert_eq!(d0.cluster_for(0), Some(cues[0].cluster_pos));
    d.seek(target.cluster_pos);
    d.push(&SMALL[usize::try_from(target.cluster_pos).unwrap()..])
        .unwrap();
    let mut first = None;
    let mut n = 0;
    while let Some(e) = d.poll().unwrap() {
        if let Event::Packet(p) = e {
            if p.track == target.track && first.is_none() {
                first = Some(p.clone());
            }
            n += 1;
        }
    }
    let first = first.unwrap();
    assert!(first.keyframe);
    assert_eq!(first.timestamp_ns, target.time_ns as i64);
    assert_eq!(first.cluster_pos, target.cluster_pos);
    let tail = packets(&all)
        .iter()
        .filter(|p| p.cluster_pos >= target.cluster_pos)
        .count();
    assert_eq!(n, tail);
}

// ---- synthetic streams

/// An element: id bytes, size as an 8-byte vint (or unknown), body.
fn el(id: u32, body: &[u8]) -> Vec<u8> {
    let mut v = id_bytes(id);
    v.push(0x01);
    v.extend_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
    v.extend_from_slice(body);
    v
}

fn el_unknown(id: u32, body: &[u8]) -> Vec<u8> {
    let mut v = id_bytes(id);
    v.push(0xFF);
    v.extend_from_slice(body);
    v
}

fn id_bytes(id: u32) -> Vec<u8> {
    let b = id.to_be_bytes();
    let skip = b.iter().position(|&x| x != 0).unwrap_or(3);
    b[skip..].to_vec()
}

fn uint_el(id: u32, v: u64) -> Vec<u8> {
    el(id, &v.to_be_bytes())
}

fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}

fn ebml_head(doc: &[u8]) -> Vec<u8> {
    el(0x1A45_DFA3, &el(0x4282, doc))
}

fn tracks_el() -> Vec<u8> {
    let video = cat(&[
        uint_el(0xD7, 1),
        uint_el(0x83, 1),
        el(0x86, b"V_VP9"),
        el(0xE0, &cat(&[uint_el(0xB0, 64), uint_el(0xBA, 48)])),
    ]);
    let audio = cat(&[
        uint_el(0xD7, 2),
        uint_el(0x83, 2),
        el(0x86, b"A_OPUS"),
        el(
            0xE1,
            &cat(&[el(0xB5, &48000f64.to_be_bytes()), uint_el(0x9F, 2)]),
        ),
    ]);
    el(0x1654_AE6B, &cat(&[el(0xAE, &video), el(0xAE, &audio)]))
}

fn simple(track: u8, rel: i16, flags: u8, data: &[u8]) -> Vec<u8> {
    let mut b = vec![0x80 | track];
    b.extend_from_slice(&rel.to_be_bytes());
    b.push(flags);
    b.extend_from_slice(data);
    el(0xA3, &b)
}

#[test]
fn unknown_sizes_block_groups_and_lacing() {
    let mut block = vec![0x82, 0x00, 0x05, 0x00];
    block.extend_from_slice(b"opus");
    let group = el(
        0xA0,
        &cat(&[
            el(0xA1, &block),
            uint_el(0x9B, 20),
            el(0x75A2, &[]), // an empty integer is 0; the later one wins
            el(0x75A2, &[0x01, 0x00]),
            el(0xFB, &[0xFF]),
        ]),
    );
    let cluster1 = el_unknown(
        0x1F43_B675,
        &cat(&[
            uint_el(0xE7, 1000),
            simple(1, 0, 0x80, b"key"),
            simple(1, 33, 0x00, b"inter"),
            simple(2, 1, 0x02, b"laced"),
            group,
            el(0xEC, &[0; 10]),
        ]),
    );
    let cluster2 = el_unknown(
        0x1F43_B675,
        &cat(&[uint_el(0xE7, 2000), simple(1, -5, 0x80, b"k2")]),
    );
    let info = el(0x1549_A966, &uint_el(0x2A_D7B1, 1_000_000));
    let segment = el_unknown(
        0x1853_8067,
        &cat(&[
            info,
            tracks_el(),
            el(0x1254_C367, &[1, 2, 3]),
            cluster1,
            cluster2,
        ]),
    );
    let file = cat(&[ebml_head(b"webm"), segment]);
    for chunks in [&[file.len()][..], &[1], &[3, 11]] {
        let (d, ev) = run(&file, chunks).unwrap();
        assert_eq!(ev[0], Event::Header);
        assert_eq!(d.tracks().len(), 2);
        assert_eq!((d.tracks()[0].width, d.tracks()[0].height), (64, 48));
        assert_eq!(
            (d.tracks()[1].sample_rate, d.tracks()[1].channels),
            (48_000, 2)
        );
        let p = packets(&ev);
        let got: Vec<(u64, i64, bool, &[u8])> = p
            .iter()
            .map(|p| (p.track, p.timestamp_ns / 1_000_000, p.keyframe, &p.data[..]))
            .collect();
        assert_eq!(
            got,
            vec![
                (1, 1000, true, &b"key"[..]),
                (1, 1033, false, b"inter"),
                (2, 1005, false, b"opus"),
                (1, 1995, true, b"k2"),
            ]
        );
        assert_eq!(p[2].duration_ns, Some(20_000_000));
        assert_eq!(p[2].discard_padding_ns, 256);
        assert_eq!(d.skipped(), 1, "the laced block");
    }
}

#[test]
fn a_new_segment_brings_a_new_header() {
    let seg = el_unknown(
        0x1853_8067,
        &cat(&[
            tracks_el(),
            el_unknown(
                0x1F43_B675,
                &cat(&[uint_el(0xE7, 0), simple(1, 0, 0x80, b"a")]),
            ),
        ]),
    );
    let file = cat(&[ebml_head(b"webm"), seg.clone(), ebml_head(b"webm"), seg]);
    let (_, ev) = run(&file, &[7]).unwrap();
    let kinds: Vec<bool> = ev.iter().map(|e| *e == Event::Header).collect();
    assert_eq!(kinds, vec![true, false, true, false]);
}

#[test]
fn rejects_what_is_not_webm() {
    let bad_doc = cat(&[ebml_head(b"avi"), el(0x1853_8067, &[])]);
    assert_eq!(run(&bad_doc, &[100]).unwrap_err(), Error::NotWebm);
    assert_eq!(run(b"RIFF....WAVE", &[100]).unwrap_err(), Error::NotWebm);
    // A failed demuxer stays failed.
    let mut d = Demuxer::new();
    d.push(&bad_doc).unwrap();
    assert!(d.poll().is_err());
    assert!(d.poll().is_err());
}

#[test]
fn hostile_sizes_are_errors_not_panics() {
    // A block bigger than MAX_BLOCK.
    let mut big = id_bytes(0xA3);
    big.push(0x01);
    big.extend_from_slice(&(MAX_BLOCK + 1).to_be_bytes()[1..]);
    let cluster = el_unknown(0x1F43_B675, &big);
    let file = cat(&[
        ebml_head(b"webm"),
        el_unknown(0x1853_8067, &cat(&[tracks_el(), cluster])),
    ]);
    assert_eq!(run(&file, &[64]).unwrap_err(), Error::TooLarge);
    // A child that runs past its parent (and the same file with honest sizes, which is fine).
    let outer = el(0x1F43_B675, &simple(1, 0, 0x80, b"xyz"));
    let ok = cat(&[
        ebml_head(b"webm"),
        el(0x1853_8067, &cat(&[tracks_el(), outer])),
    ]);
    assert!(run(&ok, &[5]).is_ok());
    let short_cluster = {
        let mut c = id_bytes(0x1F43_B675);
        c.push(0x82); // the cluster says 2 bytes...
        c.extend_from_slice(&simple(1, 0, 0x80, b"xyz")); // ...but holds a whole block
        c
    };
    let bad = cat(&[
        ebml_head(b"webm"),
        el_unknown(0x1853_8067, &cat(&[tracks_el(), short_cluster])),
    ]);
    assert!(matches!(run(&bad, &[5]).unwrap_err(), Error::Invalid(_)));
    // Unknown size where a whole element is needed.
    let bad = cat(&[
        ebml_head(b"webm"),
        el_unknown(0x1853_8067, &el_unknown(0x1654_AE6B, &[])),
    ]);
    assert!(matches!(run(&bad, &[9]).unwrap_err(), Error::Invalid(_)));
}

#[test]
fn too_much_pending_is_refused() {
    let mut d = Demuxer::new();
    let chunk = vec![0u8; MAX_PENDING / 2 + 1];
    d.push(&ebml_head(b"webm")).unwrap();
    d.push(&chunk).unwrap();
    assert_eq!(d.push(&chunk), Err(Error::TooLarge));
}

#[test]
fn skipped_elements_are_not_buffered() {
    // A 40 MiB Void streams past without being held.
    let mut d = Demuxer::new();
    d.push(&ebml_head(b"webm")).unwrap();
    let mut seg = id_bytes(0x1853_8067);
    seg.push(0xFF);
    seg.extend_from_slice(&id_bytes(0xEC));
    seg.push(0x01);
    seg.extend_from_slice(&(40u64 << 20).to_be_bytes()[1..]);
    d.push(&seg).unwrap();
    let piece = vec![0u8; 1 << 20];
    for _ in 0..40 {
        assert_eq!(d.poll().unwrap(), None);
        d.push(&piece).unwrap();
    }
    d.push(&tracks_el()).unwrap();
    assert_eq!(d.poll().unwrap(), Some(Event::Header));
}

/// Corrupt the real file in many deterministic ways; the demuxer must never panic and must stay
/// bounded (the fuzz target does the same at scale).
#[test]
fn corrupted_files_never_panic() {
    let mut seed = 0x1234_5678_9abc_def0u64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..300 {
        let mut f = SMALL.to_vec();
        for _ in 0..(rnd() % 8 + 1) {
            let at = (rnd() as usize) % f.len();
            f[at] = rnd() as u8;
        }
        let cut = (rnd() as usize) % f.len();
        let chunk = (rnd() % 5000 + 1) as usize;
        let _ = run(&f[..cut.max(1)], &[chunk]);
    }
}

#[test]
fn opus_head_parse() {
    let mut h = b"OpusHead".to_vec();
    h.extend_from_slice(&[1, 2, 0x38, 0x01, 0x80, 0xBB, 0, 0, 0, 0, 0]);
    let o = OpusHead::parse(&h).unwrap();
    assert_eq!(
        (
            o.channels,
            o.pre_skip,
            o.input_rate,
            o.gain,
            o.mapping_family
        ),
        (2, 312, 48000, 0, 0)
    );
    assert!(OpusHead::parse(&h[..18]).is_none());
    h[8] = 0x10;
    assert!(OpusHead::parse(&h).is_none(), "major version 1");
}
