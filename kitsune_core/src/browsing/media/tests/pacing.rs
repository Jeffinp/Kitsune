use super::player::{Sink, feed, header};
use crate::browsing::media::pacing::*;
use crate::browsing::media::player::{Flow, Player};
use alloc::vec::Vec;

const MS: i64 = 1_000_000;

/// A sound clock that the device reports every `step_us` (QEMU's AC'97: 10 ms), lagging the true
/// time by up to one step.
fn coarse(true_us: u64, step_us: u64) -> i64 {
    (true_us / step_us * step_us) as i64 * 1000
}

#[test]
fn smooth_clock_moves_evenly_over_a_staircase() {
    let mut c = SmoothClock::default();
    let mut prev = None;
    let mut steps = Vec::new();
    // 2 s, asked every 1 ms (a fine timer), the sound clock moving in 10 ms steps.
    for us in (0..2_000_000u64).step_by(1000) {
        let t = c.now(coarse(us, 10_000), us);
        // Never more than one sound step away from the truth.
        assert!((t - us as i64 * 1000).abs() <= 10 * MS, "t {t} at {us}");
        if let Some(p) = prev
            && us > 500_000
        {
            steps.push(t - p);
        }
        prev = Some(t);
    }
    // Once settled, every 1 ms of system time is ~1 ms of media time (the staircase would give
    // 0 or 10 ms).
    let worst = steps.iter().map(|s| (s - MS).abs()).max().unwrap();
    assert!(worst < MS / 4, "worst step error {worst} ns");
}

#[test]
fn pictures_picked_on_the_smooth_clock_are_even() {
    // 30 fps (33.333 ms) picked at every 1 ms of system time, against the 10 ms staircase and
    // against the smooth clock: the staircase gives 30/30/40, the smooth clock 33/33/34.
    let frame = 33_333_333i64;
    let spread = |smooth: bool| {
        let mut c = SmoothClock::default();
        let mut next = 0i64;
        let mut shown = Vec::new();
        for us in (0..3_000_000u64).step_by(1000) {
            let raw = coarse(us, 10_000);
            let t = if smooth { c.now(raw, us) } else { raw };
            if t >= next {
                shown.push(us);
                next += frame;
            }
        }
        let mut gaps: Vec<u32> = shown[5..]
            .windows(2)
            .map(|w| (w[1] - w[0]) as u32)
            .collect();
        Spread::of(&mut gaps)
    };
    let stair = spread(false);
    let smooth = spread(true);
    assert!(stair.sd > 4000, "staircase sd {}", stair.sd);
    assert!(smooth.sd < 1000, "smooth sd {}", smooth.sd);
    assert!(smooth.max <= 35_000 && smooth.min >= 32_000, "{smooth:?}");
}

#[test]
fn smooth_clock_waits_for_a_sound_that_stops() {
    let mut c = SmoothClock::default();
    for us in (0..500_000u64).step_by(1000) {
        c.now(us as i64 * 1000, us);
    }
    // The sound stops at 500 ms: the clock goes at most MAX_LEAD past it.
    let mut last = 0;
    for us in (500_000..900_000u64).step_by(1000) {
        last = c.now(500 * MS, us);
    }
    assert!(last <= 500 * MS + MAX_LEAD_NS, "{last}");
    assert!(last >= 500 * MS);
    // It never went back.
    assert!(c.now(500 * MS, 900_000) >= last);
}

#[test]
fn smooth_clock_jumps_on_a_big_difference_and_resets() {
    let mut c = SmoothClock::default();
    for us in (0..100_000u64).step_by(4000) {
        c.now(us as i64 * 1000, us);
    }
    // A jump forward of 2 s (a stall caught up): followed at once.
    assert_eq!(c.now(2_100 * MS, 104_000), 2_100 * MS);
    c.reset();
    assert!(!c.active());
    // After a reset (a seek back) the sound clock's value is taken as it is.
    assert_eq!(c.now(10 * MS, 108_000), 10 * MS);
    assert!(c.active());
}

#[test]
fn spread_of_intervals() {
    let mut v = alloc::vec![40_000u32, 40_000, 44_000, 36_000, 40_000];
    let s = Spread::of(&mut v);
    assert_eq!(s.n, 5);
    assert_eq!(s.mean, 40_000);
    // Variance (0 + 0 + 16e6 + 16e6 + 0) / 5 = 6.4e6: sd 2529.
    assert_eq!(s.sd, 2529);
    assert_eq!((s.min, s.max, s.p50), (36_000, 44_000, 40_000));
    assert_eq!(s.p95, 44_000);
    assert_eq!(Spread::of(&mut []), Spread::default());
    for x in [0u64, 1, 2, 3, 4, 15, 16, 17, 99, 100, 1 << 40, u64::MAX] {
        let r = isqrt(x);
        assert!(
            r * r <= x && (r + 1).checked_mul(r + 1).is_none_or(|q| q > x),
            "{x}"
        );
    }
}

#[test]
fn frame_times_fps_spread_lateness_and_log_line() {
    let mut f = FrameTimes::default();
    // 25 fps for 3 s, one picture 10 ms late every 25.
    for i in 0..75u64 {
        let jitter = if i % 25 == 24 { 10_000 } else { 0 };
        f.push(Shown {
            at_us: 1_000_000 + i * 40_000 + jitter,
            pts: i as i64 * 40 * MS,
            late_us: (jitter / 2) as i32,
        });
    }
    let now = 1_000_000 + 74 * 40_000;
    // 2 s window: 51 pictures (both ends) = 25.5 fps.
    assert_eq!(f.milli_fps(now, 2_000_000), 25_500);
    let s = f.spread(now, 2_000_000);
    assert_eq!(s.max, 50_000);
    assert_eq!(s.min, 30_000);
    assert_eq!(s.p50, 40_000);
    assert!(f.mean_late(now, 2_000_000) > 0);
    let line = f.take_line().unwrap();
    assert!(
        line.starts_with("at 1000.0 pts 0: 40.0/40/0 40.0/40/0"),
        "{line}"
    );
    assert_eq!(line.matches(' ').count(), 3 + 74);
    assert!(f.take_line().is_none());
    // The next line starts where this one ended.
    f.push(Shown {
        at_us: now + 40_000,
        pts: 75 * 40 * MS,
        late_us: 0,
    });
    assert_eq!(f.take_line().unwrap(), "at 3970.0 pts 2960: 30.0/40/0");
    f.reset();
    assert_eq!(f.milli_fps(now, 2_000_000), 0);
}

#[test]
fn player_shows_30fps_evenly_on_a_sound_clock_that_moves_in_10ms_steps() {
    let mut p: Player<u32> = Player::new(Flow::default(), true);
    let mut s = Sink::new();
    p.header(header(true, true));
    feed(&mut p, 0, 3000);
    p.step_us(0, Some(&mut s));
    p.play();
    let (mut shown, mut last) = (Vec::new(), None);
    // Asked every 1 ms; the device takes its sound in 10 ms pieces (QEMU's AC'97).
    for us in (0..2_500_000u64).step_by(1000) {
        if us % 10_000 == 0 {
            s.run(480);
        }
        p.step_us(us, Some(&mut s));
        assert!(p.next_due_ns().is_none_or(|d| d <= 34_000_000));
        if p.shown().copied() != last {
            last = p.shown().copied();
            shown.push(us);
        }
    }
    let mut gaps: Vec<u32> = shown
        .windows(2)
        .filter(|w| w[0] > 500_000)
        .map(|w| (w[1] - w[0]) as u32)
        .collect();
    let sp = Spread::of(&mut gaps);
    assert!(sp.n > 50, "{sp:?}");
    assert!(
        sp.sd < 1000 && sp.min >= 32_000 && sp.max <= 35_000,
        "{sp:?}"
    );
}

#[test]
fn the_statistics_overlay_in_both_languages() {
    use crate::browsing::media::nerd::{NerdStats, quality_name};
    use crate::i18n::Lang;
    use crate::i18n::testlang::LangGuard;
    let mut iv = alloc::vec![40_000u32, 40_100, 39_900, 44_000];
    let n = NerdStats {
        video_w: 426,
        video_h: 240,
        video_milli_fps: 25_000,
        picture_w: 426,
        picture_h: 240,
        box_w: 853,
        box_h: 480,
        shown_milli_fps: 24_900,
        spread: Spread::of(&mut iv),
        shown: 1000,
        dropped: 12,
        decoder_dropped: 3,
        late_us: 5_300,
        cpu_decoder: 45,
        cpu_compositor: 25,
        cpu_page: 20,
        budget: 2_600_000,
        ahead_ms: 12_300,
        frames_ready: 6,
        sound_clock: true,
    };
    assert_eq!(quality_name(426, 240), "240p");
    assert_eq!(quality_name(1280, 720), "720p");
    let mut g = LangGuard::new(Lang::Pt);
    let pt = n.lines();
    assert_eq!(pt.len(), 11);
    assert_eq!(pt[1], "Vídeo: 426x240 a 25,0 fps");
    assert_eq!(pt[3], "Na tela: 24,9 fps (últimos 2 s)");
    assert!(
        pt[4].starts_with("Intervalo entre quadros: 41,0 ms ± 1,7 ms"),
        "{}",
        pt[4]
    );
    assert_eq!(pt[5], "Descartados: 12 de 1.012 (3 pelo decodificador)");
    assert_eq!(pt[6], "Atraso da imagem sobre o som: 5,3 ms");
    assert_eq!(
        pt[8],
        "Qualidade: 240p (orçamento do decodificador 2,6 Mpx/s)"
    );
    assert_eq!(pt[9], "Fila: 12,3 s à frente, 6 quadros prontos");
    g.set(Lang::En);
    let en = n.lines();
    assert_eq!(en[5], "Dropped: 12 of 1,012 (3 by the decoder)");
    assert_eq!(en[7], "CPU: decoder 45%, compositor 25%, page 20%");
    let unmeasured = NerdStats { budget: 0, ..n };
    assert!(unmeasured.lines()[8].contains("not measured"));
}

#[test]
fn a_lead_picks_the_pictures_early_by_the_time_they_take_to_reach_the_screen() {
    let mut p: Player<u32> = Player::new(Flow::default(), true);
    let mut s = Sink::new();
    p.header(header(true, true));
    feed(&mut p, 0, 1000);
    p.step_us(0, Some(&mut s));
    p.play();
    p.step_us(1000, Some(&mut s));
    // Frame 3 is at 100 ms; with a 10 ms lead it goes when the clock reaches 90 ms.
    p.set_lead(10 * MS);
    s.run(89 * 48);
    p.step_us(2000, Some(&mut s));
    assert_eq!(p.shown(), Some(&2));
    s.run(48);
    p.step_us(3000, Some(&mut s));
    assert_eq!(p.shown(), Some(&3));
    // The next one (133 ms) is to be picked at 123 ms: ~33 ms from the clock at 90 ms.
    let d = p.next_due_ns().unwrap();
    assert!(d > 30 * MS && d < 35 * MS, "{d}");
    // Never more than half a frame.
    p.set_lead(100 * MS);
    let d2 = p.next_due_ns().unwrap();
    assert!(d2 >= d - 7 * MS, "{d2}");
}
