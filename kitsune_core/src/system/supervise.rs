//! Restart policy for supervised kernel threads: how long to wait before bringing a dead one back, and
//! when to stop trying.
//!
//! A thread that dies for a reason that never goes away (a bad state on disk, a parser bug a page keeps
//! triggering) would, restarted at once and without limit, burn the whole CPU in a loop of failures. So each
//! restart is delayed, the delays grow, and a fourth death inside the window means "give up": the service
//! stays dead and the user is told. The kernel supplies the clock and does the restarting; this is only the
//! decision, tested on the host.

/// Deaths remembered (and so restarts allowed) inside one window.
pub const MAX_RESTARTS: usize = 3;
/// The window, in milliseconds.
pub const WINDOW_MS: u64 = 60_000;
/// Wait before the first, second and third restart.
pub const BACKOFF_MS: [u64; MAX_RESTARTS] = [1_000, 5_000, 30_000];

/// What to do about a death.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Restart no earlier than `at_ms` (a point on the same clock as `now_ms`). `attempt` is 1-based: the
    /// first, second or third restart inside the window.
    RestartAt { at_ms: u64, attempt: usize },
    /// Too many deaths inside the window: leave the service dead.
    GiveUp,
}

/// The deaths of one service inside the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RestartBudget {
    deaths: [u64; MAX_RESTARTS],
    count: usize,
}

impl RestartBudget {
    pub const fn new() -> Self {
        Self {
            deaths: [0; MAX_RESTARTS],
            count: 0,
        }
    }

    /// Records a death at `now_ms` and says what to do. Deaths older than [`WINDOW_MS`] are forgotten, so a
    /// service that ran well for a minute starts over with the short delay. A clock that goes backwards
    /// never makes a death look older than it is.
    pub fn on_death(&mut self, now_ms: u64) -> Verdict {
        let mut kept = 0;
        for i in 0..self.count {
            let d = self.deaths[i];
            if now_ms.saturating_sub(d) < WINDOW_MS || d > now_ms {
                self.deaths[kept] = d;
                kept += 1;
            }
        }
        self.count = kept;
        if self.count >= MAX_RESTARTS {
            return Verdict::GiveUp;
        }
        let attempt = self.count + 1;
        self.deaths[self.count] = now_ms;
        self.count += 1;
        Verdict::RestartAt {
            at_ms: now_ms.saturating_add(BACKOFF_MS[attempt - 1]),
            attempt,
        }
    }

    /// Restarts still allowed inside the window (as of the last death recorded).
    pub const fn left(&self) -> usize {
        MAX_RESTARTS - self.count
    }
}

#[cfg(test)]
mod tests;
