use super::*;

#[test]
fn status_round_trips() {
    let s = AudioStatus {
        queued_bytes: 0x0102_0304,
        free_bytes: 7,
        played: 0x1122_3344_5566_7788,
        consumed: u64::MAX,
        written: 1 << 40,
        underruns: 3,
        rate: 44_100,
        channels: 2,
        flags: status_flags(55, true, false),
    };
    let b = s.to_bytes();
    assert_eq!(&b[0..4], &[4, 3, 2, 1]);
    assert_eq!(&b[8..16], &[0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11]);
    assert_eq!(AudioStatus::from_bytes(&b), s);
    assert_eq!(s.volume(), 55);
    assert!(s.paused());
    assert!(!s.draining());
}

#[test]
fn flags_and_clock() {
    let f = status_flags(100, false, true);
    let s = AudioStatus {
        flags: f,
        rate: 48_000,
        played: 72_000,
        ..AudioStatus::default()
    };
    assert_eq!((s.volume(), s.paused(), s.draining()), (100, false, true));
    assert_eq!(s.played_ms(), 1_500);
    assert_eq!(AudioStatus::default().played_ms(), 0, "no division by zero");
}

#[test]
fn numbers_stay_in_their_range() {
    // 56–57 belong to the browser link and 66–69 to another subsystem: sound is exactly 60–65.
    let all = [
        SYS_AUDIO_OPEN,
        SYS_AUDIO_WRITE,
        SYS_AUDIO_STATUS,
        SYS_AUDIO_CLOSE,
        SYS_AUDIO_CTL,
        SYS_AUDIO_WAIT,
    ];
    for (i, n) in all.iter().enumerate() {
        assert_eq!(*n, 60 + i as u64);
    }
}
