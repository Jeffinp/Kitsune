//! A line in the log for each stretch of playback (from `playing` to `pause`/`ended`/`waiting`):
//! how long it played, the pictures shown and dropped (the frame rate), the CPU the decoder and
//! the whole system used meanwhile, and how the sound clock compared with the system clock.

use super::*;
use kitsune_core::media::player::Event;

/// Where a stretch of playback began.
pub(super) struct Mark {
    ticks: u64,
    busy: [u64; kitsune_core::sysmon::MAX_THREADS],
    presented: u32,
    dropped: u32,
    position: i64,
}

impl MediaPlayer {
    /// Note the events of a step in the log.
    pub(super) fn note(&mut self, events: &[Event]) {
        for e in events {
            match e {
                Event::Playing => {
                    self.mark = Some(Mark {
                        ticks: crate::interrupts::ticks(),
                        busy: crate::sched::busy_ticks(),
                        presented: self.player.presented,
                        dropped: self.player.dropped,
                        position: self.player.position(),
                    });
                    crate::klog!(
                        Info,
                        "media: element {} playing at {} ms",
                        self.uid,
                        self.player.position() / 1_000_000
                    );
                }
                Event::Pause | Event::Ended | Event::Waiting => {
                    self.report(e.name());
                    if *e == Event::Ended {
                        crate::klog!(Info, "media: element {} ended", self.uid);
                    }
                }
                Event::Seeked => crate::klog!(
                    Info,
                    "media: element {} seeked to {} ms",
                    self.uid,
                    self.player.position() / 1_000_000
                ),
                Event::Error => crate::klog!(
                    Warn,
                    "media: element {} failed (code {})",
                    self.uid,
                    self.player.state().error.unwrap_or(0)
                ),
                _ => {}
            }
        }
    }

    /// The stretch since the last `playing`, in one log line.
    fn report(&mut self, why: &str) {
        let Some(m) = self.mark.take() else { return };
        let ticks = crate::interrupts::ticks().saturating_sub(m.ticks).max(1);
        let ms = ticks * 1000 / u64::from(crate::interrupts::TIMER_HZ);
        let busy = crate::sched::busy_ticks();
        let total: u64 = busy
            .iter()
            .zip(&m.busy)
            .map(|(a, b)| a.saturating_sub(*b))
            .sum();
        let decoder = crate::userproc::slot_of(self.pid)
            .and_then(|s| Some(busy.get(s)?.saturating_sub(*m.busy.get(s)?)))
            .unwrap_or(0);
        let compositor = busy[0].saturating_sub(m.busy[0]);
        let page = crate::userproc::slot_of(self.js)
            .and_then(|s| Some(busy.get(s)?.saturating_sub(*m.busy.get(s)?)))
            .unwrap_or(0);
        let shown = self.player.presented.saturating_sub(m.presented);
        let dropped = self.player.dropped.saturating_sub(m.dropped);
        let media_ms = (self.player.position() - m.position).max(0) / 1_000_000;
        crate::klog!(
            Info,
            "media: element {} played {} ms of media in {} ms ({}): {} pictures shown ({} fps), {} dropped; \
             CPU decoder {}%, compositor {}%, page {}%, all {}%; clock {} ({})",
            self.uid,
            media_ms,
            ms,
            why,
            shown,
            u64::from(shown) * 1000 / ms.max(1),
            dropped,
            decoder * 100 / ticks,
            compositor * 100 / ticks,
            page * 100 / ticks,
            total * 100 / ticks,
            if self.player.audio_clock() {
                "sound"
            } else {
                "system"
            },
            (media_ms as u64 * 1000)
                .checked_div(ms)
                .unwrap_or(0)
                .min(9999)
        );
    }
}
