//! The statistics overlay over a playing video ("Estatísticas do vídeo"): Shift+I over a page with a
//! video, or the item of the page's context menu over the video, turns it on and off (it starts
//! off). The browser draws it in the top-left corner of every video box, from what it measures
//! itself (`pace.rs`, the player, the scheduler's CPU ticks); the lines and their language come
//! from `kitsune_core::media::nerd`.

use super::*;
use crate::text::{self, Weight};
use core::sync::atomic::{AtomicBool, Ordering};
use kitsune_core::media::nerd::NerdStats;

/// The overlay is on (for every video: it is a tool for looking at playback, not a page setting).
static ON: AtomicBool = AtomicBool::new(false);

/// Turn the overlay on or off; returns whether it is on now.
pub(crate) fn toggle_stats() -> bool {
    let on = !ON.load(Ordering::Relaxed);
    ON.store(on, Ordering::Relaxed);
    crate::klog!(
        Info,
        "media: statistics overlay {}",
        if on { "on" } else { "off" }
    );
    on
}

/// Is the overlay on?
pub(crate) fn stats_on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// The window the overlay's rates are measured over.
const WINDOW_US: u64 = 2_000_000;

/// CPU of the decoder, the compositor and the page over the last second (percent), and where the
/// last second started (ticks, and the busy ticks of each).
#[derive(Default)]
pub(super) struct CpuShare {
    pub pct: [u8; 3],
    mark: Option<(u64, [u64; 3])>,
}

impl MediaPlayer {
    /// The busy ticks of the decoder, the compositor and the page now.
    fn busy3(&self) -> [u64; 3] {
        let busy = crate::sched::busy_ticks();
        let of = |slot: Option<usize>| slot.and_then(|s| busy.get(s).copied()).unwrap_or(0);
        [
            of(crate::userproc::slot_of(self.pid)),
            busy[0],
            of(crate::userproc::slot_of(self.js)),
        ]
    }

    /// Once a second: the CPU each part took since the last call.
    pub(super) fn sample_cpu(&mut self) {
        let now = crate::interrupts::ticks();
        let b = self.busy3();
        if let Some((t0, b0)) = self.cpu.mark {
            let dt = now.saturating_sub(t0).max(1);
            for i in 0..3 {
                self.cpu.pct[i] = (b[i].saturating_sub(b0[i]) * 100 / dt).min(100) as u8;
            }
        }
        self.cpu.mark = Some((now, b));
    }

    /// What the overlay shows for a box of `bw` x `bh`.
    fn nerd(&self, bw: i32, bh: i32) -> NerdStats {
        let now = pace::now_us();
        let t = &self.pace.times;
        let meta = self.player.state().meta;
        let dur = self.player.frame_duration();
        NerdStats {
            video_w: meta.map_or(0, |m| m.width),
            video_h: meta.map_or(0, |m| m.height),
            video_milli_fps: if dur > 0 {
                (1_000_000_000_000 / dur) as u32
            } else {
                0
            },
            picture_w: self.size.0,
            picture_h: self.size.1,
            box_w: bw.max(0) as u32,
            box_h: bh.max(0) as u32,
            shown_milli_fps: t.milli_fps(now, WINDOW_US),
            spread: t.spread(now, WINDOW_US),
            shown: self.player.presented,
            dropped: self.player.dropped,
            decoder_dropped: self.dec_dropped,
            late_us: t.mean_late(now, WINDOW_US),
            cpu_decoder: self.cpu.pct[0],
            cpu_compositor: self.cpu.pct[1],
            cpu_page: self.cpu.pct[2],
            budget: speed::budget().unwrap_or(0),
            ahead_ms: (self.ahead().unwrap_or(0).max(0) / 1_000_000) as u32,
            frames_ready: self.player.queued_frames() as u32,
            sound_clock: self.player.audio_clock(),
        }
    }
}

/// Draw the overlay of `m` in the top-left corner of the video box `r`.
pub(super) fn paint_stats(c: &mut Canvas, r: Rect, m: &MediaPlayer) {
    let lines = m.nerd(r.w, r.h).lines();
    let px: u16 = if r.w >= 600 { 12 } else { 10 };
    let lh = text::line_height(px);
    let w = lines
        .iter()
        .map(|l| text::measure(l, px, Weight::Regular))
        .max()
        .unwrap_or(0)
        + 16;
    let h = lh * lines.len() as i32 + 12;
    let panel = Rect::new(r.x + 8, r.y + 8, w.min(r.w - 16), h.min(r.h - 16));
    if panel.w <= 0 || panel.h <= 0 {
        return;
    }
    c.blend_rect(panel, Color::rgb(0, 0, 0), 190);
    let saved = c.set_clip(panel.intersection(&c.clip_rect()).unwrap_or(panel));
    for (i, l) in lines.iter().enumerate() {
        let weight = if i == 0 {
            Weight::Semibold
        } else {
            Weight::Regular
        };
        text::draw(
            c,
            panel.x + 8,
            panel.y + 6 + lh * i as i32,
            l,
            px,
            weight,
            Color::rgb(0xF2, 0xF2, 0xF2),
        );
    }
    c.restore_clip(saved);
}
