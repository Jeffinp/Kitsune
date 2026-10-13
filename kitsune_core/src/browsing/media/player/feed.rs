//! The sound side of the player: the device stream follows the player's state (paused, volume,
//! flushed on a seek), and the decoded sound goes into its queue in time order. A gap in the sound
//! (a missing packet, two segments that do not meet) is filled with silence and an overlap is cut,
//! so a sample's place in the stream is always its time: that is what lets the stream's "played"
//! counter be the clock.

use super::*;

/// Silence written in pieces of this many bytes.
const ZEROS: [u8; 4096] = [0; 4096];

impl<F> Player<F> {
    /// Bring the device stream in line with the player: flushed after a seek, paused unless the
    /// player runs, at the page's volume.
    pub(super) fn sync_sink(&mut self, s: &mut dyn AudioSink) {
        if core::mem::take(&mut self.flush_sink) {
            s.flush();
        }
        let pause = !self.st.running();
        if pause != self.sink_paused {
            s.set_paused(pause);
            self.sink_paused = pause;
        }
        let v = if self.st.muted { 0 } else { self.st.volume };
        if self.volume_set != Some(v) {
            s.set_volume(v);
            self.volume_set = Some(v);
        }
    }

    /// Put as much decoded sound into the device queue as fits.
    pub(super) fn feed(&mut self, s: &mut dyn AudioSink) {
        // Silence owed for a gap goes first.
        self.feed_silence_then(s);
        if self.silence > 0 {
            return;
        }
        while let Some(p) = self.pcm.front() {
            let frames = (p.data.len() / 4) as i64;
            if self.pcm_off == 0 {
                match self.audio_base {
                    None => {
                        // The first sound after a seek or the start: the clock is anchored here.
                        self.audio_base = Some(p.pts);
                        self.audio_written = 0;
                        self.clock.follow_audio(p.pts, s.consumed() + s.queued());
                    }
                    Some(base) => {
                        let expected = base + frames_ns(self.audio_written);
                        if p.pts > expected + AUDIO_SLACK_NS {
                            // A gap: silence up to the piece's time.
                            self.silence = ns_frames((p.pts - expected).min(MAX_GAP_NS));
                            self.feed_silence_then(s);
                            if self.silence > 0 {
                                return;
                            }
                        } else if p.pts + AUDIO_SLACK_NS < expected {
                            // An overlap: skip what was already played.
                            let skip = ns_frames(expected - p.pts).min(frames);
                            self.pcm_off = skip as usize * 4;
                        }
                    }
                }
            }
            let Some(p) = self.pcm.front() else { break };
            if self.pcm_off < p.data.len() {
                let n = s.write(&p.data[self.pcm_off..]);
                self.pcm_off += n * 4;
                self.audio_written += n as i64;
                if self.pcm_off < p.data.len() {
                    return;
                }
            }
            if let Some(done) = self.pcm.pop_front() {
                self.held_audio -= done.bytes;
                self.ack_audio += done.bytes;
            }
            self.pcm_off = 0;
        }
    }

    /// Write the silence owed now (as much as fits).
    fn feed_silence_then(&mut self, s: &mut dyn AudioSink) {
        while self.silence > 0 {
            let want = (self.silence as usize * 4).min(ZEROS.len());
            let n = s.write(&ZEROS[..want]);
            if n == 0 {
                return;
            }
            self.silence -= n as i64;
            self.audio_written += n as i64;
        }
    }
}
