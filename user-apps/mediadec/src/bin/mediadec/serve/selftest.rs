//! The server driven in memory: [`Loopback`] is a link whose sends land in a list, [`selftest`]
//! plays a whole file through it (`mediadec --serve-test`, and the host checker).

use super::*;
use kitsune_core::media::proto::{Out, parse_out};

/// An in-memory link: what is in `inbox` comes in, what the server sends lands in `outbox`.
#[derive(Default)]
pub struct Loopback {
    pub inbox: VecDeque<Vec<u8>>,
    pub outbox: Vec<Vec<u8>>,
}

impl Link for Loopback {
    fn recv(&mut self, buf: &mut [u8], _timeout_ms: u64) -> Result<Option<usize>, ()> {
        let Some(m) = self.inbox.pop_front() else {
            return Ok(None);
        };
        let n = m.len().min(buf.len());
        buf[..n].copy_from_slice(&m[..n]);
        Ok(Some(n))
    }

    fn send(&mut self, msg: &[u8]) -> Sent {
        self.outbox.push(msg.to_vec());
        Sent::Done
    }
}

/// What [`selftest`] saw.
#[derive(Debug, Default)]
pub struct Report {
    pub header: bool,
    pub pictures: u32,
    pub picture_size: (u32, u32),
    pub samples: u64,
    /// Pictures the drained message counted.
    pub end_pictures: u32,
    /// The end of what the source buffer held, ms.
    pub buffered_end_ms: i64,
    pub buffered_ranges: usize,
    /// The seek's mark came before anything of the new place.
    pub mark_first: bool,
    pub after_seek_pictures: u32,
    pub first_after_seek_ns: i64,
    /// Pictures sent with a budget of two pictures and no acknowledgement, then after one.
    pub budget_held: u32,
    pub budget_after_ack: u32,
    pub errors: u32,
}

impl Report {
    /// Everything as it must be.
    pub fn ok(&self) -> bool {
        self.header
            && self.pictures > 0
            && self.picture_size == (160, 90)
            && self.end_pictures == self.pictures
            && self.errors == 0
            && self.samples > 0
            && self.buffered_ranges == 1
            && self.buffered_end_ms >= 900
            && self.mark_first
            && self.after_seek_pictures > 0
            && self.first_after_seek_ns == 1_000_000_000
            && self.budget_held == 2
            && self.budget_after_ack > self.budget_held
    }
}

/// What [`selftest_mse`] saw.
#[derive(Debug, Default)]
pub struct MseReport {
    pub headers: u32,
    pub pictures: u32,
    pub samples: u64,
    /// What each source holds at the end, ms.
    pub video_ranges: Vec<(i64, i64)>,
    pub audio_ranges: Vec<(i64, i64)>,
    /// The sound was never more than this far behind the pictures in the output (ms): the two
    /// sources are interleaved by time.
    pub max_lag_ms: i64,
    pub drained: bool,
    /// After the video was appended a second time (a new init segment and the same clusters).
    pub ranges_after_again: Vec<(i64, i64)>,
    pub first_after_seek_ns: i64,
    pub errors: u32,
}

impl MseReport {
    pub fn ok(&self) -> bool {
        self.headers == 2
            && self.pictures == 90
            && self.samples >= 143_000
            && self.video_ranges.len() == 1
            && self.audio_ranges.len() == 1
            && self.video_ranges[0].1 >= 2_900
            && self.audio_ranges[0].1 >= 2_900
            && self.max_lag_ms <= 200
            && self.drained
            && self.ranges_after_again == self.video_ranges
            // The picture that holds 2 s (the files start 7 ms in: the sound's codec delay).
            && (1_966_000_000..=2_000_000_000).contains(&self.first_after_seek_ns)
            && self.errors == 0
    }
}

/// Media Source Extensions in memory: `video` (a WebM with only the video track) appended to source
/// 0 and `audio` (only the sound) to source 1, in pieces of odd sizes, interleaved; then the video
/// again from its init segment (another quality, the same frames), the end of the stream, and a seek
/// to 2 s.
pub fn selftest_mse(video: &[u8], audio: &[u8]) -> MseReport {
    let mut r = MseReport::default();
    let mut link = Loopback::default();
    let mut s = Server::new();
    link.inbox.push_back(proto::msg(
        proto::IN_SIZE,
        &[Field::U32(160), Field::U32(90)],
    ));
    let (mut vi, mut ai, mut seq) = (0usize, 0usize, 1u32);
    while vi < video.len() || ai < audio.len() {
        let vn = (video.len() - vi).min(12_345);
        if vn > 0 {
            link.inbox.push_back(proto::data(0, seq, &video[vi..vi + vn]));
            vi += vn;
            seq += 1;
        }
        let an = (audio.len() - ai).min(7_777);
        if an > 0 {
            link.inbox.push_back(proto::data(1, seq, &audio[ai..ai + an]));
            ai += an;
            seq += 1;
        }
    }
    link.inbox.push_back(proto::msg(proto::IN_EOS, &[Field::U8(1)]));
    pump(&mut s, &mut link);
    let mut last_audio = i64::MIN;
    for m in core::mem::take(&mut link.outbox) {
        match parse_out(&m) {
            Some(Out::Header(_)) => r.headers += 1,
            Some(Out::Frame { pts, .. }) => {
                r.pictures += 1;
                if last_audio > i64::MIN {
                    r.max_lag_ms = r.max_lag_ms.max((pts - last_audio) / 1_000_000);
                }
            }
            Some(Out::Audio { pts, frames, .. }) => {
                r.samples += u64::from(frames);
                last_audio = pts;
            }
            Some(Out::Buffered { src, ranges, .. }) => {
                let ms: Vec<(i64, i64)> = ranges
                    .iter()
                    .map(|&(a, b)| (a / 1_000_000, b / 1_000_000))
                    .collect();
                if src == 0 {
                    r.video_ranges = ms;
                } else {
                    r.audio_ranges = ms;
                }
            }
            Some(Out::Drained { .. }) => r.drained = true,
            Some(Out::Error { .. }) | None => r.errors += 1,
            _ => {}
        }
    }
    // The video again: a new init segment resets the parser; the clusters replace the same frames.
    link.inbox.push_back(proto::msg(
        proto::IN_RESET,
        &[Field::U8(0), Field::U64(u64::MAX)],
    ));
    link.inbox.push_back(proto::data(0, seq, video));
    link.inbox.push_back(proto::msg(
        proto::IN_SEEK,
        &[Field::U32(1), Field::I64(2_000_000_000)],
    ));
    pump(&mut s, &mut link);
    let mut marked = false;
    for m in core::mem::take(&mut link.outbox) {
        match parse_out(&m) {
            Some(Out::Buffered { src: 0, ranges, .. }) => {
                r.ranges_after_again = ranges
                    .iter()
                    .map(|&(a, b)| (a / 1_000_000, b / 1_000_000))
                    .collect();
            }
            Some(Out::Mark { epoch: 1 }) => marked = true,
            Some(Out::Frame { pts, .. }) if marked && r.first_after_seek_ns == 0 => {
                r.first_after_seek_ns = pts;
            }
            Some(Out::Error { .. }) | None => r.errors += 1,
            _ => {}
        }
    }
    r
}

/// Run the server until it has nothing left to do.
fn pump(s: &mut Server, link: &mut Loopback) {
    let mut buf = vec![0u8; MAX_MSG];
    loop {
        while let Ok(Some(n)) = link.recv(&mut buf, 0) {
            s.handle(&buf[..n], 0);
        }
        s.flush(link);
        if !s.can_work() {
            break;
        }
        s.work(0);
    }
    s.flush(link);
}

fn data_msgs(link: &mut Loopback, file: &[u8]) {
    for (i, c) in file.chunks(32 * 1024).enumerate() {
        link.inbox.push_back(proto::data(0, i as u32 + 1, c));
    }
}

/// Drive the server over the WebM file `file`: pictures of 160x90, the file in 32 KiB appends, the
/// end of the stream, a seek to 1 s, and the budget (two pictures, then one acknowledged).
pub fn selftest(file: &[u8]) -> Report {
    let mut r = Report::default();
    let mut link = Loopback::default();
    let mut s = Server::new();
    let size = proto::msg(proto::IN_SIZE, &[Field::U32(160), Field::U32(90)]);
    link.inbox.push_back(size.clone());
    data_msgs(&mut link, file);
    link.inbox.push_back(proto::msg(proto::IN_EOS, &[Field::U8(1)]));
    pump(&mut s, &mut link);
    for m in core::mem::take(&mut link.outbox) {
        match parse_out(&m) {
            Some(Out::Header(_)) => r.header = true,
            Some(Out::Frame { w, h, .. }) => {
                r.pictures += 1;
                r.picture_size = (w, h);
            }
            Some(Out::Audio { frames, .. }) => r.samples += u64::from(frames),
            Some(Out::Buffered { ranges, .. }) => {
                r.buffered_ranges = ranges.len();
                r.buffered_end_ms = ranges.last().map_or(0, |x| x.1 / 1_000_000);
            }
            Some(Out::Drained { .. }) if m.len() >= 9 => {
                r.end_pictures = u32::from_le_bytes([m[5], m[6], m[7], m[8]]);
            }
            Some(Out::Error { .. }) | None => r.errors += 1,
            _ => {}
        }
    }
    // A seek inside what is buffered: nothing new is sent, the frames are there.
    link.inbox.push_back(proto::msg(
        proto::IN_SEEK,
        &[Field::U32(1), Field::I64(1_000_000_000)],
    ));
    pump(&mut s, &mut link);
    let mut marked = false;
    for m in core::mem::take(&mut link.outbox) {
        match parse_out(&m) {
            Some(Out::Mark { epoch: 1 }) => {
                marked = true;
                r.mark_first = r.after_seek_pictures == 0;
            }
            Some(Out::Frame { pts, .. }) if marked => {
                if r.after_seek_pictures == 0 {
                    r.first_after_seek_ns = pts;
                }
                r.after_seek_pictures += 1;
            }
            Some(Out::Error { .. }) | None => r.errors += 1,
            _ => {}
        }
    }
    // The budget: a fresh server allowed two pictures' worth sends two and waits.
    let mut s = Server::new();
    let pic = (proto::FRAME_HEAD + 160 * 90 * 4) as u32;
    link.inbox.push_back(size);
    link.inbox.push_back(proto::msg(
        proto::IN_FLOW,
        &[Field::U32(pic * 2), Field::U32(1 << 20)],
    ));
    data_msgs(&mut link, file);
    pump(&mut s, &mut link);
    let count = |out: &[Vec<u8>]| out.iter().filter(|m| m.first() == Some(&proto::OUT_FRAME)).count() as u32;
    r.budget_held = count(&link.outbox);
    link.inbox.push_back(proto::msg(proto::IN_ACK, &[Field::U32(pic), Field::U32(0)]));
    pump(&mut s, &mut link);
    r.budget_after_ack = count(&link.outbox);
    r
}
