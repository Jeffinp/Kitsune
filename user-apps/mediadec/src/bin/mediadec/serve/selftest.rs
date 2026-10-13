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

/// Run the server until it has nothing left to do.
fn pump(s: &mut Server, link: &mut Loopback) {
    let mut buf = vec![0u8; MAX_MSG];
    loop {
        while let Ok(Some(n)) = link.recv(&mut buf, 0) {
            s.handle(&buf[..n]);
        }
        s.flush(link);
        if !s.can_work() {
            break;
        }
        s.work();
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
