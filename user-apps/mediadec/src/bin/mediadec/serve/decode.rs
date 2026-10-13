//! Decoding ahead of the player: one frame per call of [`Server::work`], the sound or the picture
//! whose time comes first, each within its budget.

use super::*;

/// How far the decoded sound may run ahead of the decoded pictures.
const AUDIO_LEAD_NS: i64 = 1_500_000_000;
/// A picture that ends this long before the player's clock is not converted nor sent...
const LATE_NS: i64 = 40_000_000;
/// ...unless no picture went for this long (a decoder slower than the video still shows some).
const LATE_EVERY_MS: u64 = 200;

impl Server {
    /// The next video frame to decode (its index and time), if the budget allows it.
    fn next_video(&self) -> Option<(usize, i64)> {
        let v = self.video.as_ref()?;
        let buf = &self.sources.get(v.src)?.video.as_ref()?.1;
        let i = buf.next(&v.cursor)?;
        let f = buf.get(i)?;
        // A picture before the seek target is decoded but never sent: no budget needed.
        let shown = f.pts + f.dur > self.target;
        let room = self.flow.is_none_or(|(b, _)| self.out_video < b);
        (!shown || room).then_some((i, f.pts))
    }

    /// The next sound frame to decode, if the budget allows it.
    fn next_audio(&self) -> Option<(usize, i64)> {
        let a = self.audio.as_ref()?;
        let buf = &self.sources.get(a.src)?.audio.as_ref()?.1;
        let i = buf.next(&a.cursor)?;
        let f = buf.get(i)?;
        let room = self.flow.is_none_or(|(_, b)| self.out_audio < b);
        room.then_some((i, f.pts - a.delay_ns))
    }

    /// Everything up to the end of the stream was decoded and nothing is waiting to go.
    fn drain_due(&self) -> bool {
        if !self.eos || self.drained_sent || !self.pending.is_empty() {
            return false;
        }
        let video_done = self.video.as_ref().is_none_or(|v| {
            self.sources
                .get(v.src)
                .and_then(|s| s.video.as_ref())
                .is_none_or(|(_, b)| b.next(&v.cursor).is_none())
        });
        let audio_done = self.audio.as_ref().is_none_or(|a| {
            self.sources
                .get(a.src)
                .and_then(|s| s.audio.as_ref())
                .is_none_or(|(_, b, _)| b.next(&a.cursor).is_none())
        });
        video_done && audio_done
    }

    /// Something can be done now.
    pub(super) fn can_work(&self) -> bool {
        self.pending.is_empty()
            && (self.next_video().is_some() || self.next_audio().is_some() || self.drain_due())
    }

    /// Decode one frame (or tell the player the stream is drained); `now_ms` is the link's clock.
    pub(super) fn work(&mut self, now_ms: u64) {
        if !self.pending.is_empty() {
            return;
        }
        match (self.next_video(), self.next_audio()) {
            (Some((i, vt)), Some((j, at))) => {
                // The sound first, up to a lead over the pictures: it is cheap and it is the
                // player's clock (a slow picture is dropped, a missing sound stops everything).
                if at <= vt.saturating_add(AUDIO_LEAD_NS) {
                    self.decode_audio(j)
                } else {
                    self.decode_video(i, now_ms)
                }
            }
            (Some((i, _)), None) => self.decode_video(i, now_ms),
            (None, Some((j, _))) => self.decode_audio(j),
            (None, None) => {
                if self.drain_due() {
                    self.drained_sent = true;
                    let m = proto::msg(
                        proto::OUT_DRAINED,
                        &[
                            Field::U32(self.epoch),
                            Field::U32(self.pictures),
                            Field::U32(self.audio_packets),
                        ],
                    );
                    self.post(m, KIND_OTHER);
                }
            }
        }
    }

    /// The player's clock now, if it runs.
    fn clock_now(&self, now_ms: u64) -> Option<i64> {
        let (t, running, at) = self.clock;
        running.then(|| t + (now_ms.saturating_sub(at).min(10_000) as i64) * 1_000_000)
    }

    fn decode_video(&mut self, i: usize, now_ms: u64) {
        let clock = self.clock_now(now_ms);
        let Server {
            sources,
            video,
            size,
            target,
            bgr,
            late,
            last_frame_ms,
            ..
        } = self;
        let Some(v) = video.as_mut() else { return };
        let Some(f) = sources
            .get(v.src)
            .and_then(|s| s.video.as_ref())
            .and_then(|(_, b)| b.get(i))
        else {
            return;
        };
        let (pts, dur) = (f.pts, f.dur);
        let r = v.dec.decode(&f.data);
        v.cursor.advance(f);
        if let Err(code) = r {
            let mut t = String::new();
            let _ = write!(t, "video frame at {} ms: {}", pts / 1_000_000, v.dec.error());
            let _ = code;
            self.error(proto::ERR_VIDEO, &t);
            return;
        }
        if pts + dur <= *target {
            return;
        }
        // Too late to be shown: the decoder has caught up only if this is not converted.
        if clock.is_some_and(|c| pts + dur < c - LATE_NS)
            && now_ms.saturating_sub(*last_frame_ms) < LATE_EVERY_MS
        {
            *late += 1;
            return;
        }
        *last_frame_ms = now_ms;
        let Some(pic) = v.dec.picture() else { return };
        let (w, h) = proto::fit(*size, pic.width(), pic.height());
        // The message as words, so the pixels are written in place (4-aligned after the head).
        let head_words = proto::FRAME_HEAD / 4;
        let mut words = vec![0u32; head_words + (w * h) as usize];
        let mut head = Vec::with_capacity(proto::FRAME_HEAD);
        proto::frame_head(&mut head, pts, w, h, *bgr);
        for (k, c) in head.chunks_exact(4).enumerate() {
            words[k] = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        }
        pic.to_pixels(&mut words[head_words..], w, h, *bgr);
        self.pictures += 1;
        self.pending.push_back((Msg::Words(words), KIND_VIDEO));
    }

    fn decode_audio(&mut self, j: usize) {
        let Server {
            sources,
            audio,
            pcm,
            target,
            ..
        } = self;
        let Some(a) = audio.as_mut() else { return };
        let Some(f) = sources
            .get(a.src)
            .and_then(|s| s.audio.as_ref())
            .and_then(|(_, b, _)| b.get(j))
        else {
            return;
        };
        let (pts, discard) = (f.pts, f.discard_ns);
        let r = a.dec.decode(&f.data, pcm);
        a.cursor.advance(f);
        let n = match r {
            Ok(n) => n,
            Err(code) => {
                let mut t = String::new();
                let _ = write!(t, "audio frame at {} ms: error {code}", pts / 1_000_000);
                self.error(proto::ERR_AUDIO, &t);
                return;
            }
        };
        // The samples' times: the block's time less the codec delay (the pre-skip).
        let t0 = pts - a.delay_ns;
        let mut head = a.skip.min(n);
        a.skip -= head;
        if *target > t0 {
            let k = ((*target - t0) as u64 * 48_000).div_ceil(1_000_000_000);
            head = head.max(k.min(n as u64) as usize);
        }
        let tail = if discard > 0 {
            ((discard as u64).saturating_mul(48_000) + 500_000_000) / 1_000_000_000
        } else {
            0
        };
        let end = n.saturating_sub(tail.min(n as u64) as usize);
        if head >= end {
            return;
        }
        let ts = t0 + (head as i64) * 1_000_000_000 / 48_000;
        let frames = end - head;
        let mut m = Vec::with_capacity(13 + frames * 4);
        proto::audio_head(&mut m, ts, frames as u32);
        for s in &pcm[head * 2..end * 2] {
            m.extend_from_slice(&s.to_le_bytes());
        }
        self.audio_packets += 1;
        self.pending.push_back((Msg::Bytes(m), KIND_AUDIO));
    }
}
