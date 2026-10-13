use super::*;
use alloc::vec;
use alloc::vec::Vec;

fn le(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

fn mix_n(m: &mut Mixer, n: usize) -> Vec<Frame> {
    let mut out = vec![[0i16; 2]; n];
    m.mix(&mut out);
    out
}

/// Zero crossings (from negative to non-negative) of the left channel.
fn rising(xs: &[Frame]) -> usize {
    xs.windows(2)
        .filter(|w| w[0][0] < 0 && w[1][0] >= 0)
        .count()
}

#[test]
fn ring_wraps_and_mono_goes_to_both_sides() {
    let mut r = SampleRing::new(1, 4);
    assert_eq!(r.push_samples(&[1, 2, 3]), 3);
    r.drop_front(2);
    assert_eq!(r.push_samples(&[4, 5, 6, 7]), 3, "only three fit");
    assert_eq!(r.len(), 4);
    assert_eq!(r.get(0), Some([3, 3]));
    assert_eq!(r.get(3), Some([6, 6]));
    assert_eq!(r.get(4), None);
    let mut s = SampleRing::new(2, 2);
    assert_eq!(
        s.push_le_bytes(&le(&[1, -1, 2, -2, 3])),
        8,
        "partial frames are not taken"
    );
    assert_eq!(s.get(1), Some([2, -2]));
    assert_eq!(s.room(), 0);
}

#[test]
fn open_checks_rate_channels_and_limits() {
    let mut m = Mixer::new();
    assert_eq!(m.open(7, 7_999, 2), Err(AudioErr::BadRate));
    assert_eq!(m.open(7, 192_000, 2), Err(AudioErr::BadRate));
    assert_eq!(m.open(7, 48_000, 3), Err(AudioErr::BadChannels));
    let ids: Vec<u32> = (0..MAX_STREAMS_PER_OWNER)
        .map(|_| m.open(7, 44_100, 2).unwrap())
        .collect();
    assert_eq!(m.open(7, 44_100, 2), Err(AudioErr::TooMany));
    // Another owner cannot touch them.
    assert_eq!(m.write(8, ids[0], &[0; 4]), Err(AudioErr::NoStream));
    assert_eq!(m.close(8, ids[0], false), Err(AudioErr::NoStream));
    for o in 100..103 {
        for _ in 0..MAX_STREAMS_PER_OWNER {
            m.open(o, 8000, 1).unwrap();
        }
    }
    assert_eq!(
        m.open(200, 8000, 1),
        Err(AudioErr::TooMany),
        "all slots taken"
    );
    assert_eq!(m.close_owner(7), MAX_STREAMS_PER_OWNER);
    // A reused slot gets a new id: the old one is dead.
    let fresh = m.open(9, 48_000, 2).unwrap();
    assert!(!ids.contains(&fresh));
    assert_eq!(m.status(7, ids[0]), Err(AudioErr::NoStream));
}

#[test]
fn queue_limit_and_full() {
    let mut m = Mixer::new();
    let id = m.open(1, 48_000, 2).unwrap();
    let big = vec![0u8; QUEUE_BYTES + 100];
    assert_eq!(m.write(1, id, &big), Ok(QUEUE_BYTES));
    assert_eq!(m.write(1, id, &[0; 4]), Err(AudioErr::Full));
    let st = m.status(1, id).unwrap();
    assert_eq!((st.queued_bytes, st.free_bytes), (QUEUE_BYTES as u64, 0));
    mix_n(&mut m, 1000);
    assert_eq!(m.write(1, id, &[0; 8]), Ok(8));
}

#[test]
fn passthrough_at_device_rate_and_saturation() {
    let mut m = Mixer::new();
    let a = m.open(1, 48_000, 2).unwrap();
    let b = m.open(2, 48_000, 1).unwrap();
    m.write(1, a, &le(&[30_000, -30_000, 100, 200])).unwrap();
    m.write(2, b, &le(&[10_000, 5])).unwrap();
    let out = mix_n(&mut m, 4);
    assert_eq!(out[0], [32_767, -20_000], "clips instead of wrapping");
    assert_eq!(out[1], [105, 205]);
    assert_eq!(out[2], [0, 0]);
    assert!(m.clipped >= 1);
}

#[test]
fn volumes_and_mute() {
    let mut m = Mixer::new();
    let a = m.open(1, 48_000, 1).unwrap();
    m.write(1, a, &le(&[20_000; 8])).unwrap();
    m.set_volume(1, a, 50).unwrap();
    assert_eq!(m.set_volume(1, a, 101), Err(AudioErr::BadValue));
    assert_eq!(mix_n(&mut m, 1)[0], [10_000, 10_000]);
    m.set_master(50).unwrap();
    assert_eq!(mix_n(&mut m, 1)[0], [5_000, 5_000]);
    m.set_muted(true);
    let st0 = m.status(1, a).unwrap().consumed;
    assert_eq!(mix_n(&mut m, 1)[0], [0, 0]);
    assert_eq!(
        m.status(1, a).unwrap().consumed,
        st0 + 1,
        "mute keeps the clock running"
    );
    m.set_muted(false);
    m.set_master(100).unwrap();
    m.set_volume(1, a, 0).unwrap();
    assert_eq!(mix_n(&mut m, 1)[0], [0, 0]);
}

#[test]
fn resampling_keeps_the_pitch_and_duration() {
    for rate in [8_000u32, 16_000, 22_050, 44_100, 48_000, 96_000] {
        let mut m = Mixer::new();
        let id = m.open(1, rate, 1).unwrap();
        // One second of 1 kHz at the stream's rate, fed in pieces as a player would.
        let mut t = Tone::new(1000, rate, 1000, 16_000);
        let mut out = Vec::new();
        let mut buf = vec![0i16; 4096];
        loop {
            let n = t.fill(&mut buf);
            let mut sent = 0;
            while sent < n {
                sent += m.write(1, id, &le(&buf[sent..n])).unwrap_or(0) / 2;
                if sent < n {
                    out.extend(mix_n(&mut m, 480));
                }
            }
            if n == 0 {
                break;
            }
        }
        while m.has_audio() {
            out.extend(mix_n(&mut m, 480));
        }
        let first = out.iter().position(|f| f[0] != 0).unwrap();
        let last = out.iter().rposition(|f| f[0] != 0).unwrap();
        let secs_x1000 = (last - first) as u64 * 1000 / 48_000;
        assert!(
            (995..=1005).contains(&secs_x1000),
            "{rate}: {secs_x1000} ms"
        );
        let cycles = rising(&out);
        assert!((995..=1001).contains(&cycles), "{rate}: {cycles} cycles");
        // Interpolation must not invent jumps: neighbours stay close for a 1 kHz tone.
        let worst = out
            .windows(2)
            .map(|w| (i32::from(w[0][0]) - i32::from(w[1][0])).abs())
            .max()
            .unwrap();
        assert!(worst < 2_200, "{rate}: step {worst}");
    }
}

#[test]
fn played_follows_the_device_not_the_mixer() {
    let mut m = Mixer::new();
    let id = m.open(1, 24_000, 2).unwrap();
    m.write(1, id, &vec![1u8; 24_000 * 4]).unwrap(); // 1 s
    mix_n(&mut m, 9_600); // 200 ms handed to the device
    let st = m.status(1, id).unwrap();
    assert_eq!(st.consumed, 4_800);
    assert_eq!(st.played, 0, "nothing heard yet");
    m.set_device_played(4_800); // 100 ms heard
    assert_eq!(m.status(1, id).unwrap().played, 2_400);
    m.set_device_played(9_600);
    assert_eq!(m.status(1, id).unwrap().played, 4_800);
    // A device that claims more than it was given is clamped.
    m.set_device_played(1_000_000);
    assert_eq!(m.device_played(), 9_600);
}

#[test]
fn clock_survives_gaps_and_late_streams() {
    let mut m = Mixer::new();
    mix_n(&mut m, 1_000); // device time passes with nothing open
    m.set_device_played(1_000);
    let id = m.open(1, 48_000, 1).unwrap();
    m.write(1, id, &le(&[5; 300])).unwrap();
    mix_n(&mut m, 1_000); // 300 frames of the stream, then silence
    let st = m.status(1, id).unwrap();
    assert_eq!(
        (st.consumed, st.underruns),
        (300, 0),
        "ending is not an underrun"
    );
    m.set_device_played(1_150);
    assert_eq!(m.status(1, id).unwrap().played, 150);
    m.set_device_played(1_600);
    assert_eq!(m.status(1, id).unwrap().played, 300);
    assert!(m.drained(1, id).unwrap());
    assert!(m.ready(1, id, 0));
    assert!(m.ready(1, id, 4096));
    assert!(!m.ready(1, id, QUEUE_BYTES as u64 + 1));
    assert!(
        m.ready(1, 12345, 0),
        "a stream that is gone wakes its waiter"
    );
    // More samples after running dry: that was a gap, an underrun.
    m.write(1, id, &le(&[5; 10])).unwrap();
    assert_eq!(m.status(1, id).unwrap().underruns, 1);
    // Paused: no frames taken, no underrun.
    m.set_paused(1, id, true).unwrap();
    mix_n(&mut m, 100);
    let st = m.status(1, id).unwrap();
    assert_eq!((st.consumed, st.underruns, st.paused), (300, 1, true));
    assert!(!m.has_audio());
}

#[test]
fn many_segments_keep_the_clock_monotonic() {
    let mut m = Mixer::new();
    let id = m.open(1, 44_100, 2).unwrap();
    let mut last = 0;
    for step in 0..400u64 {
        let _ = m.write(1, id, &vec![0u8; 4 * 441]);
        mix_n(&mut m, 480);
        // The device lags by ~12 blocks of 512 frames, like the AC'97 driver.
        m.set_device_played((step * 480).saturating_sub(6_144));
        let p = m.status(1, id).unwrap().played;
        assert!(p >= last, "clock went back at {step}");
        last = p;
    }
    let dev = m.device_played();
    assert!(last.abs_diff(dev * 44_100 / 48_000) <= 2, "{last} vs {dev}");
}

#[test]
fn drain_close_and_flush() {
    let mut m = Mixer::new();
    let id = m.open(3, 48_000, 1).unwrap();
    m.write(3, id, &le(&[7; 100])).unwrap();
    m.close(3, id, true).unwrap();
    assert_eq!(
        m.write(3, id, &le(&[1])),
        Err(AudioErr::NoStream),
        "no writes after close"
    );
    let out = mix_n(&mut m, 150);
    assert_eq!(out[99], [7, 7]);
    assert_eq!(out[100], [0, 0]);
    assert_eq!(m.open_count(), 0, "gone once played out");
    // Immediate close drops what is queued.
    let id = m.open(3, 48_000, 1).unwrap();
    m.write(3, id, &le(&[7; 100])).unwrap();
    m.close(3, id, false).unwrap();
    assert_eq!(mix_n(&mut m, 10)[0], [0, 0]);
    // Flush keeps the stream and its clock.
    let id = m.open(3, 48_000, 1).unwrap();
    m.write(3, id, &le(&[7; 100])).unwrap();
    mix_n(&mut m, 10);
    m.flush(3, id).unwrap();
    assert_eq!(mix_n(&mut m, 10)[0], [0, 0]);
    assert_eq!(m.status(3, id).unwrap().consumed, 10);
}

#[test]
fn sine_table_is_accurate() {
    // Against a reference computed with f64 on the host.
    let mut worst = 0i32;
    for k in 0..4096u32 {
        let phase = k.wrapping_mul(1_048_576).wrapping_add(k * 37);
        let x = f64::from(phase) / 4_294_967_296.0 * core::f64::consts::TAU;
        let want = (x.sin() * 32767.0).round() as i32;
        worst = worst.max((i32::from(tone::sine(phase)) - want).abs());
    }
    assert!(worst <= 2, "worst error {worst} LSB");
    assert_eq!(tone::sine(0), 0);
    assert_eq!(tone::sine(0x4000_0000), 32767);
    assert_eq!(tone::sine(0xC000_0000), -32767);
}

#[test]
fn tone_has_its_frequency_length_and_fades() {
    let mut t = Tone::new(440, 48_000, 500, 20_000);
    let mut buf = vec![0i16; 30_000];
    let n = t.fill(&mut buf);
    assert_eq!(n, 24_000);
    assert_eq!(t.fill(&mut buf), 0);
    let frames: Vec<Frame> = buf[..n].iter().map(|&s| [s, s]).collect();
    assert_eq!(rising(&frames), 220);
    assert_eq!(buf[0], 0, "fades in");
    assert!(buf[n - 1].abs() < 300, "fades out");
    let peak = buf[..n].iter().map(|s| s.abs()).max().unwrap();
    assert!((19_900..=20_000).contains(&peak), "{peak}");
}
