use super::*;

#[test]
fn round_robin_order() {
    assert_eq!(next_runnable(0, 3, |_| true), Some(1));
    assert_eq!(next_runnable(1, 3, |_| true), Some(2));
    assert_eq!(next_runnable(2, 3, |_| true), Some(0));
}

#[test]
fn skips_blocked_threads() {
    assert_eq!(next_runnable(0, 3, |i| i != 1), Some(2));
    assert_eq!(next_runnable(2, 3, |i| i != 0), Some(1));
}

#[test]
fn current_is_the_last_candidate() {
    // Only the current thread can run: it keeps the CPU.
    assert_eq!(next_runnable(1, 3, |i| i == 1), Some(1));
}

#[test]
fn dead_current_is_never_picked() {
    // The current thread is not runnable (just died): another one is chosen.
    assert_eq!(next_runnable(2, 3, |i| i != 2), Some(0));
    assert_eq!(next_runnable(1, 3, |i| i == 0 || i == 2), Some(2));
    // Nobody else is runnable: no answer instead of resuming the dead thread.
    assert_eq!(next_runnable(2, 3, |_| false), None);
}

#[test]
fn single_slot_and_empty() {
    assert_eq!(next_runnable(0, 1, |_| true), Some(0));
    assert_eq!(next_runnable(0, 1, |_| false), None);
    assert_eq!(next_runnable(0, 0, |_| true), None);
}

#[test]
fn wraps_with_a_cur_outside_the_range() {
    // `cur` is a slot index; a stale value larger than n still terminates.
    assert_eq!(next_runnable(5, 3, |_| true), Some(0));
}

#[test]
fn a_held_slot_waits_while_others_can_run() {
    // Slot 2 is held: from 1 the next is 0, not 2.
    assert_eq!(next_runnable_held(1, 4, |_| true, |i| i == 2), Some(3));
    assert_eq!(next_runnable_held(3, 4, |_| true, |i| i == 2), Some(0));
    // Only the current thread and the held one can run: the held one runs (the current one is
    // giving the CPU away, or its slice ended).
    assert_eq!(
        next_runnable_held(1, 4, |i| i == 1 || i == 2, |i| i == 2),
        Some(2)
    );
    // Only the held one can run.
    assert_eq!(next_runnable_held(0, 4, |i| i == 2, |i| i == 2), Some(2));
    // Nothing held: plain round robin.
    assert_eq!(next_runnable_held(1, 4, |_| true, |_| false), Some(2));
    assert_eq!(next_runnable_held(1, 4, |_| false, |_| false), None);
}
