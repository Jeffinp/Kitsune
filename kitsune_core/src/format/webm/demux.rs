//! The incremental demuxer: a buffer of pushed bytes, the open masters (segment, cluster) and a
//! loop that reads one element header at a time.

use super::ebml::{self, Children, UNKNOWN};
use super::header;
use super::ids::*;
use super::*;
use core::ops::Range;

/// An open master element whose children stream past (segment or cluster).
#[derive(Clone, Copy, Debug)]
struct Level {
    id: u32,
    /// Absolute offset where it ends (for an unknown size, its parent's end, if known).
    end: Option<u64>,
    /// Its size was unknown: an id that cannot be its child also ends it.
    unknown: bool,
}

/// The WebM demuxer; see the module documentation.
#[derive(Debug, Default)]
pub struct Demuxer {
    buf: Vec<u8>,
    /// Bytes of `buf` already consumed.
    pos: usize,
    /// Absolute file offset of `buf[0]`.
    base: u64,
    /// Bytes of an element being skipped that have not arrived yet.
    skip: u64,
    levels: Vec<Level>,
    /// The first element was seen (it must be the EBML header).
    started: bool,
    failed: Option<Error>,
    info: Info,
    tracks: Vec<Track>,
    cues: Vec<CuePoint>,
    /// Absolute offset of the segment's data (cue positions count from it) and its end, if known.
    segment_start: u64,
    segment_end: Option<u64>,
    cluster_time: u64,
    cluster_pos: u64,
    skipped: u64,
}

impl Demuxer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget everything (a new stream follows).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Append `data` to what is waiting to be read. [`Error::TooLarge`] (nothing is taken) when
    /// more than [`MAX_PENDING`] bytes would wait: poll first.
    pub fn push(&mut self, data: &[u8]) -> Result<(), Error> {
        self.compact(true);
        let pending = self.buf.len() - self.pos;
        if pending.saturating_add(data.len()) > MAX_PENDING {
            return Err(Error::TooLarge);
        }
        self.buf.extend_from_slice(data);
        Ok(())
    }

    /// The segment's `Info` (defaults until it is read).
    pub fn info(&self) -> &Info {
        &self.info
    }

    /// The tracks of the current segment (empty until [`Event::Header`]).
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// The track numbered `number`.
    pub fn track(&self, number: u64) -> Option<&Track> {
        self.tracks.iter().find(|t| t.number == number)
    }

    /// The cue points read so far, by time.
    pub fn cues(&self) -> &[CuePoint] {
        &self.cues
    }

    /// Blocks dropped because they were laced.
    pub fn skipped(&self) -> u64 {
        self.skipped
    }

    /// Absolute file offset of the next byte to be read.
    pub fn position(&self) -> u64 {
        self.base + self.pos as u64
    }

    /// The file offset of the cluster to start from to show `time_ns`: the last cue point at or
    /// before it (the first one if `time_ns` comes before all). `None` without cues.
    pub fn cluster_for(&self, time_ns: u64) -> Option<u64> {
        let after = self.cues.partition_point(|c| c.time_ns <= time_ns);
        self.cues
            .get(after.saturating_sub(1))
            .map(|c| c.cluster_pos)
    }

    /// Drop what is buffered and read the bytes pushed next as starting at file offset `offset`,
    /// at the segment level (a cluster, normally from [`Demuxer::cluster_for`]). The header and
    /// cues stay; a failure is cleared.
    pub fn seek(&mut self, offset: u64) {
        self.buf.clear();
        self.pos = 0;
        self.base = offset;
        self.skip = 0;
        self.failed = None;
        self.started = true;
        self.cluster_time = 0;
        self.levels.clear();
        self.levels.push(Level {
            id: SEGMENT,
            end: self.segment_end,
            unknown: self.segment_end.is_none(),
        });
    }

    /// The next header or packet; `Ok(None)` when more bytes are needed.
    pub fn poll(&mut self) -> Result<Option<Event>, Error> {
        loop {
            if let Some(e) = self.failed {
                return Err(e);
            }
            if self.skip > 0 {
                let n = self.skip.min((self.buf.len() - self.pos) as u64);
                self.pos += n as usize;
                self.skip -= n;
                if self.skip > 0 {
                    self.compact(false);
                    return Ok(None);
                }
            }
            let abs = self.position();
            while self
                .levels
                .last()
                .is_some_and(|l| l.end.is_some_and(|e| abs >= e))
            {
                self.levels.pop();
            }
            let h = match ebml::header(&self.buf[self.pos..]) {
                Ok(Some(h)) => h,
                Ok(None) => {
                    self.compact(false);
                    return Ok(None);
                }
                Err(e) => return self.fail(e),
            };
            if let Some(l) = self.levels.last().copied()
                && l.unknown
            {
                let foreign = match l.id {
                    CLUSTER => !is_cluster_child(h.id),
                    _ => ends_segment(h.id),
                };
                if foreign {
                    self.levels.pop();
                    continue;
                }
            }
            let data_start = abs + h.len as u64;
            let parent_end = self.levels.last().and_then(|l| l.end);
            let end = if h.size == UNKNOWN {
                None
            } else {
                match data_start.checked_add(h.size) {
                    Some(e) => Some(e),
                    None => return self.fail(Error::Invalid("element size overflows")),
                }
            };
            if let (Some(e), Some(p)) = (end, parent_end)
                && e > p
            {
                return self.fail(Error::Invalid("element larger than its parent"));
            }
            if !self.started {
                if h.id != EBML {
                    return self.fail(Error::NotWebm);
                }
                self.started = true;
            }
            match self.element(h, data_start, end.or(parent_end))? {
                Step::Continue => {}
                Step::NeedData => {
                    self.compact(false);
                    return Ok(None);
                }
                Step::Event(e) => return Ok(Some(e)),
            }
        }
    }

    /// Handle the element whose header `h` is at `self.pos`.
    fn element(
        &mut self,
        h: ebml::Header,
        data_start: u64,
        end: Option<u64>,
    ) -> Result<Step, Error> {
        let parent = self.levels.last().map(|l| l.id);
        match (parent, h.id) {
            (None, EBML) => {
                let Some(range) = self.body(h, MAX_HEADER)? else {
                    return Ok(Step::NeedData);
                };
                let body = &self.buf[range];
                let r = header::ebml_header(body);
                self.done(r, h)?;
            }
            (None, SEGMENT) => {
                self.pos += h.len;
                self.segment_start = data_start;
                self.segment_end = (h.size != UNKNOWN).then_some(end).flatten();
                self.tracks.clear();
                self.cues.clear();
                self.info = Info::default();
                self.open(SEGMENT, h, end)?;
            }
            (Some(SEGMENT), CLUSTER) => {
                self.cluster_pos = data_start - h.len as u64;
                self.cluster_time = 0;
                self.pos += h.len;
                self.open(CLUSTER, h, end)?;
            }
            (Some(SEGMENT), INFO) => {
                let Some(range) = self.body(h, MAX_HEADER)? else {
                    return Ok(Step::NeedData);
                };
                let body = &self.buf[range];
                let r = header::info(body);
                let info = self.done(r, h)?;
                self.info = info;
            }
            (Some(SEGMENT), TRACKS) => {
                let Some(range) = self.body(h, MAX_HEADER)? else {
                    return Ok(Step::NeedData);
                };
                let body = &self.buf[range];
                let r = header::tracks(body);
                self.tracks = self.done(r, h)?;
                return Ok(Step::Event(Event::Header));
            }
            (Some(SEGMENT), CUES) if h.size <= MAX_CUES_BYTES => {
                let Some(range) = self.body(h, MAX_CUES_BYTES)? else {
                    return Ok(Step::NeedData);
                };
                let body = &self.buf[range];
                let mut cues = core::mem::take(&mut self.cues);
                let r = header::cues(body, self.segment_start, self.info.timescale_ns, &mut cues);
                self.cues = cues;
                self.done(r, h)?;
            }
            (Some(CLUSTER), TIMECODE) => {
                let Some(range) = self.body(h, 8)? else {
                    return Ok(Step::NeedData);
                };
                let body = &self.buf[range];
                let r = ebml::uint(body);
                self.cluster_time = self.done(r, h)?;
            }
            (Some(CLUSTER), SIMPLE_BLOCK | BLOCK_GROUP) => {
                let Some(range) = self.body(h, MAX_BLOCK)? else {
                    return Ok(Step::NeedData);
                };
                let body = &self.buf[range];
                let r = if h.id == SIMPLE_BLOCK {
                    self.block(body, None)
                } else {
                    self.group(body)
                };
                let packet = self.done(r, h)?;
                match packet {
                    Some(p) => return Ok(Step::Event(Event::Packet(p))),
                    None => self.skipped += 1,
                }
            }
            _ => {
                if h.size == UNKNOWN {
                    return self.fail(Error::Invalid(
                        "element of unknown size that cannot be skipped",
                    ));
                }
                self.pos += h.len;
                self.skip = h.size;
            }
        }
        Ok(Step::Continue)
    }

    /// Enter the master `id` whose header is at `pos` (already consumed).
    fn open(&mut self, id: u32, h: ebml::Header, end: Option<u64>) -> Result<(), Error> {
        if self.levels.len() >= 2 {
            return self.fail(Error::Invalid("masters nested too deep"));
        }
        self.levels.push(Level {
            id,
            end,
            unknown: h.size == UNKNOWN,
        });
        Ok(())
    }

    /// Where the body of the element at `pos` is in `buf`, once all of it has arrived; an error
    /// when it is larger than `limit` or of unknown size.
    fn whole(&self, h: ebml::Header, limit: u64) -> Result<Option<Range<usize>>, Error> {
        if h.size == UNKNOWN {
            return Err(Error::Invalid(
                "element of unknown size where one is needed",
            ));
        }
        if h.size > limit {
            return Err(Error::TooLarge);
        }
        let start = self.pos + h.len;
        let end = start + h.size as usize;
        Ok((end <= self.buf.len()).then_some(start..end))
    }

    /// [`Demuxer::whole`], failing the demuxer on an error.
    fn body(&mut self, h: ebml::Header, limit: u64) -> Result<Option<Range<usize>>, Error> {
        match self.whole(h, limit) {
            Ok(r) => Ok(r),
            Err(e) => self.fail(e),
        }
    }

    /// Consume the element at `pos` and pass on what parsing it gave, failing the demuxer on an
    /// error.
    fn done<T>(&mut self, r: Result<T, Error>, h: ebml::Header) -> Result<T, Error> {
        match r {
            Ok(v) => {
                self.pos += h.len + h.size as usize;
                Ok(v)
            }
            Err(e) => self.fail(e),
        }
    }

    fn fail<T>(&mut self, e: Error) -> Result<T, Error> {
        self.failed = Some(e);
        Err(e)
    }

    /// Drop consumed bytes from the front of the buffer when that is worth a copy (`force`: any).
    fn compact(&mut self, force: bool) {
        if self.pos == 0 || (!force && self.pos < 64 * 1024 && self.pos * 2 < self.buf.len()) {
            return;
        }
        self.buf.drain(..self.pos);
        self.base += self.pos as u64;
        self.pos = 0;
    }

    /// A `Block` or `SimpleBlock` body: `None` for a laced one.
    fn block(&self, b: &[u8], group: Option<GroupExtra>) -> Result<Option<Packet>, Error> {
        let Some((track, n)) = ebml::vint(b)? else {
            return Err(Error::Invalid("block cut short"));
        };
        let Some(head) = b.get(n..n + 3) else {
            return Err(Error::Invalid("block cut short"));
        };
        let rel = i16::from_be_bytes([head[0], head[1]]);
        let flags = head[2];
        if flags & 0x06 != 0 {
            return Ok(None);
        }
        let scale = i128::from(self.info.timescale_ns);
        let ts = (i128::from(self.cluster_time) + i128::from(rel)) * scale;
        let ts = ts.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
        let (keyframe, duration_ns, discard) = match group {
            None => (flags & 0x80 != 0, None, 0),
            Some(g) => (
                !g.referenced,
                g.duration.map(|d| d.saturating_mul(self.info.timescale_ns)),
                g.discard_padding,
            ),
        };
        Ok(Some(Packet {
            track,
            timestamp_ns: ts,
            keyframe,
            duration_ns,
            discard_padding_ns: discard,
            cluster_pos: self.cluster_pos,
            data: b[n + 3..].to_vec(),
        }))
    }

    fn group(&self, b: &[u8]) -> Result<Option<Packet>, Error> {
        let mut extra = GroupExtra::default();
        let mut block = None;
        for child in Children::new(b) {
            let (id, body) = child?;
            match id {
                BLOCK => block = Some(body),
                BLOCK_DURATION => extra.duration = Some(ebml::uint(body)?),
                REFERENCE_BLOCK => extra.referenced = true,
                DISCARD_PADDING => extra.discard_padding = ebml::int(body)?,
                _ => {}
            }
        }
        match block {
            Some(body) => self.block(body, Some(extra)),
            None => Err(Error::Invalid("block group without a block")),
        }
    }
}

/// What a `BlockGroup` adds to its `Block`.
#[derive(Clone, Copy, Default)]
struct GroupExtra {
    duration: Option<u64>,
    referenced: bool,
    discard_padding: i64,
}

/// What handling one element led to.
enum Step {
    Continue,
    NeedData,
    Event(Event),
}
