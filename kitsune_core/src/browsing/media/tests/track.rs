use crate::browsing::media::track::{Cursor, RANGE_SLACK_NS, TrackBuffer, opus_packet_ns};
use alloc::vec;
use alloc::vec::Vec;

const MS: i64 = 1_000_000;
/// 30 fps.
const F: i64 = 33_333_333;

/// Video frames `from..to` (frame numbers), a keyframe every `gop`, 100 bytes each.
fn video(buf: &mut TrackBuffer, from: i64, to: i64, gop: i64) {
    for n in from..to {
        buf.push(n * F, None, n % gop == 0, 0, vec![n as u8; 100]);
    }
}

fn ranges(buf: &TrackBuffer) -> Vec<(i64, i64)> {
    buf.buffered().iter().collect()
}

#[test]
fn video_frames_learn_their_duration_and_make_one_range() {
    let mut b = TrackBuffer::new(true);
    video(&mut b, 0, 90, 30);
    assert_eq!(b.len(), 90);
    assert_eq!(b.bytes(), 9000);
    assert_eq!(b.get(0).unwrap().dur, F);
    let r = ranges(&b);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].0, 0);
    // The last frame lasts as long as the one before.
    assert_eq!(r[0].1, 89 * F + F);
}

#[test]
fn a_gap_splits_the_ranges_but_rounding_does_not() {
    let mut b = TrackBuffer::new(false);
    for i in 0..50 {
        b.push(i * 20 * MS, Some(20 * MS), true, 0, vec![0; 10]);
    }
    // 1 ms short: still one range.
    for i in 0..50 {
        b.push(
            1000 * MS + 1 * MS + i * 20 * MS,
            Some(20 * MS),
            true,
            0,
            vec![0; 10],
        );
    }
    assert_eq!(ranges(&b), [(0, 2001 * MS)]);
    // A real gap.
    b.push(5000 * MS, Some(20 * MS), true, 0, vec![0; 10]);
    assert_eq!(ranges(&b).len(), 2);
    assert!(RANGE_SLACK_NS < 100 * MS);
}

#[test]
fn appending_again_replaces_and_keeps_order() {
    let mut b = TrackBuffer::new(true);
    video(&mut b, 0, 60, 30);
    // The second second appended again (a quality switch): same times, new bytes.
    for n in 30..60 {
        b.push(n * F, None, n % 30 == 0, 0, vec![200; 50]);
    }
    assert_eq!(b.len(), 60, "nothing doubled");
    assert!((0..59).all(|i| b.get(i).unwrap().pts < b.get(i + 1).unwrap().pts));
    assert_eq!(b.get(30).unwrap().data[0], 200);
    assert_eq!(b.get(29).unwrap().data[0], 29);
    assert_eq!(b.bytes(), 30 * 100 + 30 * 50);
}

#[test]
fn remove_takes_the_dependent_frames_too() {
    let mut b = TrackBuffer::new(true);
    video(&mut b, 0, 90, 30);
    // Remove frames 10..20: 20..30 depend on them and go too, up to the keyframe at 30.
    b.remove(10 * F, 20 * F);
    assert_eq!(b.len(), 70);
    let r = ranges(&b);
    assert_eq!(r, [(0, 10 * F), (30 * F, 90 * F)]);
    // Audio frames stand alone.
    let mut a = TrackBuffer::new(false);
    for i in 0..10 {
        a.push(i * 20 * MS, Some(20 * MS), false, 0, vec![1; 4]);
    }
    a.remove(40 * MS, 80 * MS);
    assert_eq!(a.len(), 8);
}

#[test]
fn eviction_keeps_what_shows_the_time() {
    let mut b = TrackBuffer::new(true);
    video(&mut b, 0, 90, 30);
    // At 1.5 s the frames from the keyframe at 1 s on are needed.
    let freed = b.evict_before(45 * F);
    assert_eq!(freed, 3000);
    assert_eq!(b.get(0).unwrap().pts, 30 * F);
    assert!(b.get(0).unwrap().key);
    // Audio: what ended before the time.
    let mut a = TrackBuffer::new(false);
    for i in 0..10 {
        a.push(i * 20 * MS, Some(20 * MS), true, 0, vec![1; 4]);
    }
    a.evict_before(50 * MS);
    assert_eq!(a.get(0).unwrap().pts, 40 * MS);
}

#[test]
fn seek_starts_at_the_keyframe_before() {
    let mut b = TrackBuffer::new(true);
    video(&mut b, 0, 90, 30);
    assert_eq!(b.seek_index(45 * F + 5), Some(30));
    assert_eq!(b.seek_index(0), Some(0));
    assert_eq!(b.seek_index(30 * F), Some(30));
    // Past the end: nothing.
    assert_eq!(b.seek_index(200 * F), None);
    // Not yet buffered, but a keyframe begins just after: it serves.
    let mut late = TrackBuffer::new(true);
    video(&mut late, 31, 60, 30);
    late.push(300 * F, None, true, 0, vec![0; 3]);
    assert_eq!(late.seek_index(29 * F), None, "frame 31 is not a keyframe");
    let mut seg = TrackBuffer::new(true);
    for n in 60..90 {
        seg.push(n * F + 2 * MS, None, n == 60, 0, vec![0; 3]);
    }
    assert_eq!(seg.seek_index(60 * F), Some(0));
    // Audio: the frame that holds the time.
    let mut a = TrackBuffer::new(false);
    for i in 0..10 {
        a.push(i * 20 * MS, Some(20 * MS), true, 0, vec![1; 4]);
    }
    assert_eq!(a.seek_index(55 * MS), Some(2));
}

#[test]
fn the_cursor_walks_and_skips_holes_to_a_keyframe() {
    let mut b = TrackBuffer::new(true);
    video(&mut b, 0, 60, 30);
    let mut c = Cursor::default();
    let mut seen = Vec::new();
    while let Some(i) = b.next(&c) {
        let f = b.get(i).unwrap().clone();
        seen.push(f.pts / F);
        c.advance(&f);
    }
    assert_eq!(seen, (0..60).collect::<Vec<_>>());
    assert!(b.done(&c));
    // More data later: the cursor goes on from where it was.
    video(&mut b, 60, 70, 30);
    assert_eq!(b.next(&c).map(|i| b.get(i).unwrap().pts / F), Some(60));

    // A hole in front of the decoder: frames 40..45 replaced by other ones (new seq), so the
    // decoder that stopped at 39 must wait for the next keyframe.
    let mut b = TrackBuffer::new(true);
    video(&mut b, 0, 90, 30);
    let mut c = Cursor::default();
    for _ in 0..40 {
        let i = b.next(&c).unwrap();
        let f = b.get(i).unwrap().clone();
        c.advance(&f);
    }
    b.remove(40 * F, 45 * F);
    let i = b.next(&c).unwrap();
    assert_eq!(b.get(i).unwrap().pts, 60 * F);

    // A seek: the cursor starts from the keyframe.
    let c = Cursor::seek(50 * F);
    let i = b.next(&c).unwrap();
    assert_eq!(
        b.get(i).unwrap().pts,
        60 * F,
        "30..60 lost its middle: the keyframe after"
    );
    let c = Cursor::seek(70 * F);
    assert_eq!(b.get(b.next(&c).unwrap()).unwrap().pts, 60 * F);
}

#[test]
fn opus_durations() {
    // Config 31 (CELT 20 ms), one frame.
    assert_eq!(opus_packet_ns(&[31 << 3]), 20 * MS);
    // Config 0 (SILK 10 ms), two frames (code 1).
    assert_eq!(opus_packet_ns(&[1]), 20 * MS);
    // Config 1 (SILK 20 ms), code 3 with 3 frames.
    assert_eq!(opus_packet_ns(&[(1 << 3) | 3, 3]), 60 * MS);
    // CELT 2.5 ms.
    assert_eq!(opus_packet_ns(&[16 << 3]), 2_500_000);
    // Capped and malformed.
    assert_eq!(opus_packet_ns(&[(3 << 3) | 3, 63]), 120 * MS);
    assert_eq!(opus_packet_ns(&[]), 0);
    assert_eq!(opus_packet_ns(&[3]), 0);
}
