//! Who gets the CPU while a video plays: the decoder (the pictures), the compositor (the screen) and
//! the sound come before the page's scripts once the page has fetched enough ahead. Then the page's
//! script process is demoted (`sched::set_demoted`): it still gets every tick nobody else wants and
//! at least one in four when the CPU is busy, which is plenty for a player that updates its
//! controls and fetches the next segment every few seconds. When what is buffered ahead runs low (the
//! page must fetch), the video pauses or ends, it is a normal thread again.

use super::*;

/// Buffered media ahead of the position from which the page's scripts can wait.
const DEMOTE_AHEAD_NS: i64 = 5_000_000_000;
/// ...and below which they come first again (a gap so the state does not flip at every append).
const RESTORE_AHEAD_NS: i64 = 3_000_000_000;

impl MediaPlayer {
    /// Note what source `src` holds (an answer of the decoder).
    pub(super) fn note_buffered(&mut self, src: u8, ranges: &[(i64, i64)]) {
        let i = usize::from(src);
        if i < proto::MAX_SOURCES {
            if self.ranges.len() <= i {
                self.ranges.resize(i + 1, Vec::new());
            }
            self.ranges[i] = ranges.to_vec();
        }
    }

    /// How much media is buffered ahead of the position (the least over the sources that hold
    /// something), if known.
    fn ahead(&self) -> Option<i64> {
        let pos = self.player.position();
        self.ranges
            .iter()
            .filter(|r| !r.is_empty())
            .map(|r| {
                r.iter()
                    .find(|&&(a, b)| a <= pos + 100_000_000 && pos < b)
                    .map_or(0, |&(_, b)| b - pos)
            })
            .min()
    }
}

impl MediaSet {
    /// Demote or restore the page's script process `js` for what its players do now.
    pub(crate) fn update_priority(&mut self, js: u32) {
        let want = self.players.iter().any(|p| {
            let playing = p.player.state().running();
            let ahead = p.ahead().unwrap_or(0);
            playing
                && if self.demoted {
                    ahead >= RESTORE_AHEAD_NS
                } else {
                    ahead >= DEMOTE_AHEAD_NS
                }
        });
        if want == self.demoted {
            return;
        }
        let Some(slot) = crate::userproc::slot_of(js) else {
            return;
        };
        self.demoted = want;
        crate::sched::set_demoted(slot, want);
        crate::klog!(
            Info,
            "media: page scripts (pid {js}) {} while the video plays",
            if want { "demoted" } else { "back to normal" }
        );
    }
}
