//! A click whose default action (follow a link, submit a form) waits for the page: its handlers may
//! call `preventDefault()`. The browser sends the click with a sequence number and keeps the action;
//! the process answers with the verdict. A page that does not answer in time (busy, or gone) does not
//! hold the click hostage: the action happens anyway.

use super::*;

/// How long a default action waits for the page's verdict, in milliseconds.
pub const ANSWER_TIMEOUT_MS: u64 = 1500;

/// The clicks of one page in flight. `A` is what the browser does when the page allows it.
pub struct Session<A> {
    next_seq: u32,
    /// `(seq, deadline in ms, action)`.
    pending: Option<(u32, u64, A)>,
}

impl<A> Default for Session<A> {
    fn default() -> Self {
        Session {
            next_seq: 1,
            pending: None,
        }
    }
}

impl<A> Session<A> {
    /// A click on element `uid` at `at`, input number `n`, at time `now_ms`. With `action` the click
    /// waits for a verdict (a click before it replaces the one waiting, which is dropped). Returns the
    /// message for the process.
    pub fn click(
        &mut self,
        uid: u32,
        at: ClickAt,
        n: u32,
        action: Option<A>,
        now_ms: u64,
    ) -> Vec<u8> {
        let seq = self.wait(action, now_ms);
        click_message(uid, at, seq, n)
    }

    /// Keep `action` until the page's verdict (a key, a click); returns the sequence number to send
    /// (0 without an action).
    pub fn wait(&mut self, action: Option<A>, now_ms: u64) -> u32 {
        match action {
            Some(a) => {
                let seq = self.next_seq;
                self.next_seq = self.next_seq.wrapping_add(1).max(1);
                self.pending = Some((seq, now_ms.saturating_add(ANSWER_TIMEOUT_MS), a));
                seq
            }
            None => 0,
        }
    }

    /// The page answered click `seq`: the action to do now, if it is the one waiting and it was
    /// allowed.
    pub fn answer(&mut self, seq: u32, allow: bool) -> Option<A> {
        match &self.pending {
            Some((s, _, _)) if *s == seq => {
                let (_, _, a) = self.pending.take()?;
                allow.then_some(a)
            }
            _ => None,
        }
    }

    /// The action whose verdict did not come by `now_ms` (it happens without one).
    pub fn expire(&mut self, now_ms: u64) -> Option<A> {
        match &self.pending {
            Some((_, deadline, _)) if now_ms >= *deadline => self.pending.take().map(|p| p.2),
            _ => None,
        }
    }

    /// The page is gone: the action waiting happens now.
    pub fn abandon(&mut self) -> Option<A> {
        self.pending.take().map(|p| p.2)
    }

    /// Is a click waiting for its verdict?
    pub fn waiting(&self) -> bool {
        self.pending.is_some()
    }
}
