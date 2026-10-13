//! What the page sees of a player (`HTMLMediaElement`): `paused`, `ended`, `seeking`, `readyState`,
//! the duration, and the events, in the order the HTML standard fires them.

use super::*;

/// The page's events, by name ([`Event::name`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    LoadedMetadata,
    DurationChange,
    Resize,
    LoadedData,
    CanPlay,
    CanPlayThrough,
    Play,
    Playing,
    Pause,
    Waiting,
    Seeking,
    Seeked,
    TimeUpdate,
    Ended,
    VolumeChange,
    Error,
}

impl Event {
    /// The DOM event type.
    pub fn name(self) -> &'static str {
        match self {
            Event::LoadedMetadata => "loadedmetadata",
            Event::DurationChange => "durationchange",
            Event::Resize => "resize",
            Event::LoadedData => "loadeddata",
            Event::CanPlay => "canplay",
            Event::CanPlayThrough => "canplaythrough",
            Event::Play => "play",
            Event::Playing => "playing",
            Event::Pause => "pause",
            Event::Waiting => "waiting",
            Event::Seeking => "seeking",
            Event::Seeked => "seeked",
            Event::TimeUpdate => "timeupdate",
            Event::Ended => "ended",
            Event::VolumeChange => "volumechange",
            Event::Error => "error",
        }
    }
}

/// What the tracks are (all the sources' headers together).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    pub video: bool,
    pub audio: bool,
    pub width: u32,
    pub height: u32,
    /// From the headers (0: unknown).
    pub duration_ns: u64,
}

/// `readyState`.
pub const HAVE_NOTHING: u8 = 0;
pub const HAVE_METADATA: u8 = 1;
pub const HAVE_CURRENT_DATA: u8 = 2;
pub const HAVE_FUTURE_DATA: u8 = 3;
pub const HAVE_ENOUGH_DATA: u8 = 4;

/// `MediaError.code` for a decode failure.
pub const MEDIA_ERR_DECODE: u8 = 3;
/// `MediaError.code` for a format the player cannot play.
pub const MEDIA_ERR_SRC_NOT_SUPPORTED: u8 = 4;

/// The state the page sees.
#[derive(Clone, Debug)]
pub struct State {
    pub meta: Option<Meta>,
    /// The duration the page sees (`None`: unknown, `Infinity`... or `NaN` before the metadata).
    pub duration: Option<i64>,
    /// What the page set (`MediaSource.duration`), which wins over the headers.
    page_duration: Option<Option<i64>>,
    pub paused: bool,
    pub ended: bool,
    /// Seeking to this time.
    pub seeking: Option<i64>,
    /// Playing but out of data.
    pub waiting: bool,
    pub ready: u8,
    /// No more data will come.
    pub eos: bool,
    /// The decoder sent everything up to the end.
    pub drained: bool,
    pub epoch: u32,
    marked_epoch: u32,
    pub looping: bool,
    pub volume: u8,
    pub muted: bool,
    pub error: Option<u8>,
    /// `loadeddata` was sent.
    loaded: bool,
    pub(in super::super) events: Vec<Event>,
}

impl Default for State {
    fn default() -> Self {
        State {
            meta: None,
            duration: None,
            page_duration: None,
            paused: true,
            ended: false,
            seeking: None,
            waiting: false,
            ready: HAVE_NOTHING,
            eos: false,
            drained: false,
            epoch: 0,
            marked_epoch: 0,
            looping: false,
            volume: 100,
            muted: false,
            error: None,
            loaded: false,
            events: Vec::new(),
        }
    }
}

impl State {
    pub(super) fn push(&mut self, e: Event) {
        // Two of the same in a row say nothing more (a burst of `timeupdate`s).
        if self.events.last() != Some(&e) || e != Event::TimeUpdate {
            self.events.push(e);
        }
    }

    /// What the decoder sends now belongs to the current seek.
    pub fn current(&self) -> bool {
        self.marked_epoch == self.epoch
    }

    pub(super) fn marked(&self) -> bool {
        self.current()
    }

    /// The clock may run.
    pub fn running(&self) -> bool {
        !self.paused
            && !self.waiting
            && !self.ended
            && self.seeking.is_none()
            && self.error.is_none()
            && self.ready >= HAVE_FUTURE_DATA
    }

    pub(super) fn header(&mut self, h: Header) {
        let mut m = self.meta.unwrap_or_default();
        let first = self.meta.is_none();
        if h.video && (!m.video || (m.width, m.height) != (h.width, h.height)) {
            if m.video {
                self.push(Event::Resize);
            }
            m.video = true;
            m.width = h.width;
            m.height = h.height;
        }
        m.audio |= h.audio;
        m.duration_ns = m.duration_ns.max(h.duration_ns);
        if !m.video && !m.audio {
            // Nothing this player can decode (yet: another source may bring it).
            if first {
                return;
            }
        }
        self.meta = Some(m);
        if self.page_duration.is_none() {
            let d = (m.duration_ns > 0).then_some(m.duration_ns.min(i64::MAX as u64) as i64);
            self.change_duration(d);
        }
        if first {
            self.ready = HAVE_METADATA;
            self.push(Event::LoadedMetadata);
        }
    }

    fn change_duration(&mut self, d: Option<i64>) {
        if self.duration != d {
            self.duration = d;
            self.push(Event::DurationChange);
        }
    }

    pub(super) fn set_duration(&mut self, d: Option<i64>) {
        self.page_duration = Some(d);
        self.change_duration(d);
    }

    pub(super) fn mark(&mut self, epoch: u32) {
        if epoch == self.epoch {
            self.marked_epoch = epoch;
        }
    }

    pub(super) fn fail(&mut self, code: u8) {
        if self.error.is_none() {
            self.error = Some(code);
            self.push(Event::Error);
        }
    }

    pub(super) fn play(&mut self) {
        if !self.paused {
            return;
        }
        self.paused = false;
        self.push(Event::Play);
        if self.ready >= HAVE_FUTURE_DATA && self.seeking.is_none() {
            self.push(Event::Playing);
        } else {
            self.waiting = true;
            self.push(Event::Waiting);
        }
    }

    pub(super) fn pause(&mut self) {
        if self.paused {
            return;
        }
        self.paused = true;
        self.waiting = false;
        self.push(Event::TimeUpdate);
        self.push(Event::Pause);
    }

    pub(super) fn seek(&mut self, t: i64) -> u32 {
        self.epoch = self.epoch.wrapping_add(1);
        self.seeking = Some(t);
        self.ended = false;
        self.drained = false;
        if self.ready > HAVE_METADATA {
            self.ready = HAVE_METADATA;
        }
        self.push(Event::Seeking);
        self.epoch
    }

    /// The new place of a seek is ready.
    pub(super) fn seek_done(&mut self) {
        if self.seeking.take().is_some() {
            self.push(Event::TimeUpdate);
            self.push(Event::Seeked);
        }
    }

    /// Move `readyState` to `r`, with the events of the way.
    fn set_ready(&mut self, r: u8) {
        let old = self.ready;
        if r == old {
            return;
        }
        self.ready = r;
        if r >= HAVE_CURRENT_DATA && !self.loaded {
            self.loaded = true;
            self.push(Event::LoadedData);
        }
        if r >= HAVE_FUTURE_DATA && old < HAVE_FUTURE_DATA {
            self.push(Event::CanPlay);
            if !self.paused && self.seeking.is_none() {
                self.waiting = false;
                self.push(Event::Playing);
            }
        }
        if r == HAVE_ENOUGH_DATA && old < HAVE_ENOUGH_DATA {
            self.push(Event::CanPlayThrough);
        }
    }

    /// One step: `enough` data to play on, `current` data for the time on screen, the data ran
    /// `dry` while playing, the stream `finished`. Returns `true` when a `loop` must start over.
    pub(super) fn step(&mut self, enough: bool, current: bool, dry: bool, finished: bool) -> bool {
        if self.error.is_some() {
            return false;
        }
        let ready = if self.meta.is_none() {
            HAVE_NOTHING
        } else if !current {
            HAVE_METADATA
        } else if enough {
            HAVE_ENOUGH_DATA
        } else if self.ready >= HAVE_FUTURE_DATA && !dry {
            // Less ahead than "enough", but not run dry: playing goes on (no flapping between
            // `canplay` and nothing while the decoder keeps up just in time).
            HAVE_FUTURE_DATA
        } else {
            HAVE_CURRENT_DATA
        };
        self.set_ready(ready);
        if self.paused || self.seeking.is_some() || self.ended {
            return false;
        }
        if finished {
            if self.looping {
                return true;
            }
            self.ended = true;
            self.paused = true;
            self.waiting = false;
            self.push(Event::TimeUpdate);
            self.push(Event::Pause);
            self.push(Event::Ended);
            return false;
        }
        if !self.waiting && dry {
            self.waiting = true;
            if self.ready > HAVE_CURRENT_DATA {
                self.ready = HAVE_CURRENT_DATA;
            }
            self.push(Event::Waiting);
        }
        false
    }
}
