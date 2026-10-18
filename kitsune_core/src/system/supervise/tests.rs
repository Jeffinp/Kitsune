use super::*;

fn restart(at_ms: u64, attempt: usize) -> Verdict {
    Verdict::RestartAt { at_ms, attempt }
}

#[test]
fn the_first_death_restarts_after_one_second() {
    let mut b = RestartBudget::new();
    assert_eq!(b.on_death(10_000), restart(11_000, 1));
    assert_eq!(b.left(), 2);
}

#[test]
fn delays_grow_with_each_death_in_the_window() {
    let mut b = RestartBudget::new();
    assert_eq!(b.on_death(0), restart(1_000, 1));
    assert_eq!(b.on_death(2_000), restart(7_000, 2));
    assert_eq!(b.on_death(8_000), restart(38_000, 3));
}

#[test]
fn the_fourth_death_in_the_window_gives_up() {
    let mut b = RestartBudget::new();
    b.on_death(0);
    b.on_death(2_000);
    b.on_death(8_000);
    assert_eq!(b.on_death(40_000), Verdict::GiveUp);
    assert_eq!(b.left(), 0);
    // and it keeps saying so while the window still holds all three
    assert_eq!(b.on_death(41_000), Verdict::GiveUp);
}

#[test]
fn deaths_older_than_the_window_are_forgotten() {
    let mut b = RestartBudget::new();
    b.on_death(0);
    b.on_death(2_000);
    b.on_death(8_000);
    // 60 s after the first death it no longer counts: one slot is free again, and the delay is the
    // second one (two deaths still inside the window)
    assert_eq!(b.on_death(WINDOW_MS), restart(WINDOW_MS + 30_000, 3));
}

#[test]
fn a_long_quiet_spell_starts_over() {
    let mut b = RestartBudget::new();
    b.on_death(0);
    b.on_death(2_000);
    b.on_death(8_000);
    assert_eq!(b.on_death(1_000_000), restart(1_001_000, 1));
    assert_eq!(b.left(), 2);
}

#[test]
fn the_window_edge_is_exclusive() {
    let mut b = RestartBudget::new();
    b.on_death(0);
    // exactly WINDOW_MS later the first death is out; one millisecond earlier it is still in
    let mut inside = b;
    assert_eq!(
        inside.on_death(WINDOW_MS - 1),
        restart(WINDOW_MS - 1 + 5_000, 2)
    );
    assert_eq!(b.on_death(WINDOW_MS), restart(WINDOW_MS + 1_000, 1));
}

#[test]
fn a_clock_that_goes_backwards_does_not_forget_deaths() {
    let mut b = RestartBudget::new();
    b.on_death(100_000);
    b.on_death(101_000);
    b.on_death(102_000);
    assert_eq!(b.on_death(5_000), Verdict::GiveUp);
}

#[test]
fn the_deadline_saturates_instead_of_wrapping() {
    let mut b = RestartBudget::new();
    assert_eq!(
        b.on_death(u64::MAX - 10),
        restart(u64::MAX, 1),
        "no overflow at the end of time"
    );
}

#[test]
fn backoff_is_increasing_and_matches_the_budget_size() {
    assert_eq!(BACKOFF_MS.len(), MAX_RESTARTS);
    assert!(BACKOFF_MS.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn random_death_times_never_panic_and_never_allow_more_than_the_budget() {
    let mut x = 0x1234_5678_9ABC_DEF1u64;
    let mut b = RestartBudget::new();
    let mut now = 0u64;
    let mut live: Vec<u64> = Vec::new();
    for _ in 0..20_000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        now += x % 40_000;
        let v = b.on_death(now);
        live.retain(|&d| now - d < WINDOW_MS);
        match v {
            Verdict::RestartAt { attempt, at_ms } => {
                assert!(live.len() < MAX_RESTARTS, "restart granted over budget");
                live.push(now);
                assert_eq!(attempt, live.len());
                assert_eq!(at_ms, now + BACKOFF_MS[attempt - 1]);
            }
            Verdict::GiveUp => assert_eq!(live.len(), MAX_RESTARTS),
        }
    }
}
