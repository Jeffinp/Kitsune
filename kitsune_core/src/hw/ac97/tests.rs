use super::*;

#[test]
fn descriptor_layout() {
    assert_eq!(
        descriptor(0x1234_5678, 1024, BD_BUP),
        [0x78, 0x56, 0x34, 0x12, 0x00, 0x04, 0x00, 0x40]
    );
}

#[test]
fn fills_up_to_target_and_counts_progress() {
    let mut r = PlayRing::new(512);
    assert_eq!(r.wanted(12), 12);
    for i in 0..12 {
        assert_eq!(r.submit(), i);
    }
    r.started();
    assert_eq!(r.wanted(12), 0);
    assert_eq!(r.wanted(40), 18, "never more than 30 queued");
    // On block 0, 100 frames in (picb counts 16-bit samples: 2 per frame).
    let o = r.observe(0, 2 * 412, 0);
    assert_eq!(
        o,
        Observed {
            played: 100,
            ran_dry: false
        }
    );
    // Three blocks later.
    let o = r.observe(3, 1024, 0);
    assert_eq!(o.played, 3 * 512);
    assert_eq!(r.queued(), 9);
    assert_eq!(r.wanted(12), 3);
}

#[test]
fn index_wraps_around_the_list() {
    let mut r = PlayRing::new(512);
    let mut civ = 0u8;
    for _ in 0..12 {
        r.submit();
    }
    r.started();
    for step in 0..200u64 {
        // The device moves 2 blocks per look; the driver tops up to 12.
        civ = (civ + 2) % 32;
        let o = r.observe(civ, 1024, 0);
        assert_eq!(o.played, (step + 1) * 2 * 512);
        for _ in 0..r.wanted(12) {
            r.submit();
        }
        assert_eq!(r.queued(), 12);
    }
}

#[test]
fn halt_means_everything_played_and_resume_at_next() {
    let mut r = PlayRing::new(512);
    for _ in 0..5 {
        r.submit();
    }
    r.started();
    r.observe(2, 600, 0);
    // The device played block 4 (the last valid) and halted.
    let o = r.observe(4, 0, SR_DCH | SR_CELV | SR_LVBCI);
    assert_eq!(
        o,
        Observed {
            played: 5 * 512,
            ran_dry: true
        }
    );
    assert!(!r.running());
    assert_eq!(r.next_index(), 5);
    // Looking again while halted is not a second underrun.
    assert!(!r.observe(4, 0, SR_DCH).ran_dry);
    // Refill and restart: the device resumes at descriptor 5.
    r.submit();
    r.started();
    let o = r.observe(5, 512, 0);
    assert_eq!(o.played, 5 * 512 + 256);
}

#[test]
fn never_started_is_not_an_underrun() {
    let mut r = PlayRing::new(512);
    assert_eq!(
        r.observe(0, 0, SR_DCH),
        Observed {
            played: 0,
            ran_dry: false
        }
    );
    assert_eq!(r.next_index(), 0);
}

#[test]
fn restart_between_looks_counts_the_finished_block() {
    // The device halts after block 1 and the driver's LVI write restarts it before the driver saw
    // the halt: the next look finds it on block 2.
    let mut r = PlayRing::new(512);
    r.submit();
    r.submit();
    r.started();
    r.observe(1, 1024, 0);
    r.submit();
    let o = r.observe(2, 1000, 0);
    assert_eq!(o.played, 2 * 512 + 12);
}
