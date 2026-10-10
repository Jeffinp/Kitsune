use super::*;
use alloc::vec::Vec;

#[test]
fn series_ring_keeps_the_last_60() {
    let mut s = Series::new();
    assert!(s.is_empty());
    assert_eq!(s.last(), None);
    for i in 0..100u32 {
        s.push(i);
    }
    assert_eq!(s.len(), HIST);
    assert_eq!(s.get(0), Some(40));
    assert_eq!(s.last(), Some(99));
    assert_eq!(s.get(HIST), None);
    assert_eq!(s.max(), 99);
    let v: Vec<u32> = s.iter().collect();
    assert_eq!(v.len(), 60);
    assert!(v.windows(2).all(|w| w[1] == w[0] + 1));
}

#[test]
fn series_stats() {
    let mut s = Series::new();
    for v in [10, 20, 30] {
        s.push(v);
    }
    assert_eq!(s.avg(), 20);
    assert_eq!(s.max(), 30);
    assert_eq!(s.moving_avg(2, 2), 25);
    assert_eq!(s.moving_avg(0, 5), 10);
    assert_eq!(s.moving_avg(2, 10), 20);
    assert_eq!(s.moving_avg(3, 2), 0);
    assert_eq!(s.moving_avg(1, 0), 0);
    assert_eq!(Series::new().avg(), 0);
    assert_eq!(Series::new().max(), 0);
}

#[test]
fn nice_ceilings() {
    assert_eq!(nice_ceiling(0, 0), 1);
    assert_eq!(nice_ceiling(1, 0), 1);
    assert_eq!(nice_ceiling(3, 0), 5);
    assert_eq!(nice_ceiling(5, 0), 5);
    assert_eq!(nice_ceiling(6, 0), 10);
    assert_eq!(nice_ceiling(101, 0), 200);
    assert_eq!(nice_ceiling(4_999, 0), 5_000);
    assert_eq!(nice_ceiling(20, 100), 100);
    assert_eq!(nice_ceiling(u64::MAX, 0), u64::MAX);
    assert!(nice_ceiling(u64::MAX / 2, 0) >= u64::MAX / 2);
}

#[test]
fn scaling() {
    assert_eq!(scale_to(50, 100, 80), 40);
    assert_eq!(scale_to(500, 100, 80), 80);
    assert_eq!(scale_to(5, 0, 80), 0);
}

#[test]
fn rate_meter() {
    let mut m = RateMeter::new();
    assert_eq!(m.rate(1000, 1), 0);
    assert_eq!(m.rate(1600, 1), 600);
    assert_eq!(m.rate(2200, 2), 300);
    assert_eq!(m.rate(10, 1), 0); // counter reset
    assert_eq!(m.rate(40, 1), 30);
}

/// Pad a short list of busy counters to all slots.
fn pad(v: &[u64]) -> [u64; MAX_THREADS] {
    let mut a = [0; MAX_THREADS];
    a[..v.len()].copy_from_slice(v);
    a
}

#[test]
fn cpu_first_sample_is_idle() {
    let mut s = CpuSampler::new();
    let r = s.sample(1000, &pad(&[500, 0, 0, 0, 0, 0, 0, 0]));
    assert_eq!(r.idle_pm, 1000);
    assert_eq!(r.busy_pm, 0);
}

#[test]
fn cpu_shares_sum_to_100_percent() {
    let mut s = CpuSampler::new();
    s.sample(0, &[0; MAX_THREADS]);
    // 250 ticks: thread 0 ran 100, thread 1 ran 50, thread 2 ran 25.
    let r = s.sample(250, &pad(&[100, 50, 25, 0, 0, 0, 0, 0]));
    assert_eq!(r.thread_pm[0], 400);
    assert_eq!(r.thread_pm[1], 200);
    assert_eq!(r.thread_pm[2], 100);
    assert_eq!(r.busy_pm, 700);
    assert_eq!(r.idle_pm, 300);
    let total: u32 = r.thread_pm.iter().map(|&p| p as u32).sum::<u32>() + r.idle_pm as u32;
    assert_eq!(total, 1000);
}

#[test]
fn cpu_sum_is_exact_for_awkward_splits() {
    let mut s = CpuSampler::new();
    s.sample(0, &[0; MAX_THREADS]);
    let r = s.sample(3, &pad(&[1, 1, 1, 0, 0, 0, 0, 0]));
    let total: u32 = r.thread_pm.iter().map(|&p| p as u32).sum::<u32>() + r.idle_pm as u32;
    assert_eq!(total, 1000);
    assert_eq!(r.idle_pm, 0);
}

#[test]
fn cpu_over_count_is_clamped() {
    let mut s = CpuSampler::new();
    s.sample(0, &[0; MAX_THREADS]);
    // Slots add up to more ticks than elapsed (racy read): still <= 100%.
    let r = s.sample(100, &pad(&[60, 60, 0, 0, 0, 0, 0, 0]));
    assert!(r.busy_pm <= 1000);
    assert_eq!(r.busy_pm + r.idle_pm, 1000);
}

#[test]
fn cpu_zero_elapsed_and_counter_regression() {
    let mut s = CpuSampler::new();
    s.sample(10, &[5; MAX_THREADS]);
    let r = s.sample(10, &[9; MAX_THREADS]);
    assert_eq!(r.idle_pm, 1000);
    let r = s.sample(20, &[0; MAX_THREADS]); // counters went backwards
    assert_eq!(r.busy_pm, 0);
}

fn row(name: &str, cpu: Option<u16>, mem: Option<u32>, up: u32) -> ProcRow {
    let mut r = ProcRow::new(name.as_bytes(), RowKind::App, 1, RowState::Running);
    r.cpu_pm = cpu;
    r.mem_kib = mem;
    r.up_s = up;
    r
}

fn names(rows: &[ProcRow]) -> Vec<&[u8]> {
    rows.iter().map(|r| r.name()).collect()
}

#[test]
fn sorting() {
    let mut rows = [
        row("shell", Some(100), None, 5),
        row("Editor", Some(300), Some(128), 50),
        row("calc", None, Some(256), 20),
    ];
    sort_rows(&mut rows, SortKey::Name, false);
    assert_eq!(names(&rows), [&b"calc"[..], b"Editor", b"shell"]);
    sort_rows(&mut rows, SortKey::Cpu, true);
    assert_eq!(names(&rows), [&b"Editor"[..], b"shell", b"calc"]);
    sort_rows(&mut rows, SortKey::Mem, true);
    assert_eq!(names(&rows), [&b"calc"[..], b"Editor", b"shell"]);
    sort_rows(&mut rows, SortKey::Up, false);
    assert_eq!(names(&rows), [&b"shell"[..], b"calc", b"Editor"]);
}

#[test]
fn sorting_is_stable() {
    let mut rows = [
        row("a", Some(10), None, 0),
        row("b", Some(10), None, 0),
        row("c", Some(10), None, 0),
    ];
    sort_rows(&mut rows, SortKey::Cpu, true);
    assert_eq!(names(&rows), [&b"a"[..], b"b", b"c"]);
    sort_rows(&mut rows, SortKey::Cpu, false);
    assert_eq!(names(&rows), [&b"a"[..], b"b", b"c"]);
}

#[test]
fn row_name_is_cut() {
    let r = ProcRow::new(
        b"a-very-long-process-name",
        RowKind::Thread,
        0,
        RowState::Idle,
    );
    assert_eq!(r.name().len(), 16);
    assert_eq!(RowState::Dead.label(), "DEAD");
}

#[test]
fn formatting() {
    assert_eq!(fmt_pct10(0).as_bytes(), b"0.0%");
    assert_eq!(fmt_pct10(123).as_bytes(), b"12.3%");
    assert_eq!(fmt_pct10(1000).as_bytes(), b"100.0%");
    assert_eq!(fmt_bytes(0).as_bytes(), b"0 B");
    assert_eq!(fmt_bytes(1023).as_bytes(), b"1023 B");
    assert_eq!(fmt_bytes(1024).as_bytes(), b"1.0 KiB");
    assert_eq!(fmt_bytes(1536).as_bytes(), b"1.5 KiB");
    assert_eq!(fmt_bytes(64 * 1024 * 1024).as_bytes(), b"64.0 MiB");
    assert_eq!(fmt_bytes(3 << 30).as_bytes(), b"3.0 GiB");
    assert_eq!(fmt_rate(2048).as_bytes(), b"2.0 KiB/s");
    assert_eq!(fmt_uptime(0).as_bytes(), b"00:00:00");
    assert_eq!(fmt_uptime(3725).as_bytes(), b"01:02:05");
    assert_eq!(fmt_uptime(90_061).as_bytes(), b"1d 01:01:01");
    // Never panics on the extremes.
    let _ = fmt_bytes(u64::MAX);
    let _ = fmt_uptime(u64::MAX);
}
