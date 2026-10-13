//! The media messages between a page's script process and the browser.
//!
//! **Page to browser** (after the kind byte, see `web::script::parse_message`):
//!
//! | kind | rest | meaning |
//! |---|---|---|
//! | `M` | `op id args...` (ASCII, space separated) | a command for player `id` ([`parse_control`]) |
//! | `m` | `id src seq` and a newline, then bytes | WebM bytes for source buffer `src` ([`parse_data`]) |
//!
//! **Browser to page**: JSON `{"t":"media","id":N,...}` with the state and the events
//! ([`state_message`]), what a source buffer holds ([`buffered_message`]) and where a cluster is
//! ([`cue_message`]). Times are nanoseconds.

use super::player::{Event, State};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write as _;

/// A command from the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// A new player for element `id` (its decoder starts).
    Open,
    /// The element let go of its media (`src` changed, `load()`, removed): the player ends.
    Close,
    Play,
    Pause,
    /// `currentTime = t`.
    Seek(i64),
    /// Volume 0–100 and mute.
    Volume(u8, bool),
    Loop(bool),
    /// `endOfStream()` (`true`), or more data after all.
    EndOfStream(bool),
    /// `MediaSource.duration` (`None`: infinite or unknown).
    Duration(Option<i64>),
    /// `SourceBuffer.remove(a, b)` on source `src`.
    Remove {
        src: u8,
        a: i64,
        b: i64,
    },
    /// The next bytes of source `src` are at file offset `offset` (`abort()`, a direct source that
    /// jumps).
    Reset {
        src: u8,
        offset: u64,
    },
    /// `SourceBuffer.timestampOffset`.
    TimestampOffset {
        src: u8,
        ns: i64,
    },
    /// Where is the cluster for time `t` in source `src`.
    Cue {
        src: u8,
        t: i64,
    },
}

/// A command and the element it is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command {
    pub id: u32,
    pub control: Control,
}

/// Read an `M` message (without the kind byte). `None` when malformed (the page is not trusted).
pub fn parse_control(m: &[u8]) -> Option<Command> {
    let text = core::str::from_utf8(m).ok()?;
    if text.len() > 200 {
        return None;
    }
    let mut it = text.split_ascii_whitespace();
    let op = it.next()?;
    let id: u32 = it.next()?.parse().ok()?;
    let mut num = || -> Option<i64> { it.next()?.parse().ok() };
    let flag = |v: i64| v != 0;
    let src = |v: i64| -> Option<u8> {
        u8::try_from(v)
            .ok()
            .filter(|&s| (s as usize) < super::proto::MAX_SOURCES)
    };
    let control = match op {
        "open" => Control::Open,
        "close" => Control::Close,
        "play" => Control::Play,
        "pause" => Control::Pause,
        "seek" => Control::Seek(num()?.max(0)),
        "vol" => Control::Volume(num()?.clamp(0, 100) as u8, flag(num()?)),
        "loop" => Control::Loop(flag(num()?)),
        "eos" => Control::EndOfStream(flag(num()?)),
        "dur" => {
            let d = num()?;
            Control::Duration((d >= 0).then_some(d))
        }
        "rm" => Control::Remove {
            src: src(num()?)?,
            a: num()?,
            b: num()?,
        },
        "reset" => Control::Reset {
            src: src(num()?)?,
            offset: u64::try_from(num()?).ok()?,
        },
        "tso" => Control::TimestampOffset {
            src: src(num()?)?,
            ns: num()?,
        },
        "cue" => Control::Cue {
            src: src(num()?)?,
            t: num()?.max(0),
        },
        _ => return None,
    };
    Some(Command { id, control })
}

/// Read an `m` message (without the kind byte): element, source, sequence number and the bytes.
pub fn parse_data(m: &[u8]) -> Option<(u32, u8, u32, &[u8])> {
    let nl = m.iter().take(48).position(|&b| b == b'\n')?;
    let head = core::str::from_utf8(&m[..nl]).ok()?;
    let mut it = head.split_ascii_whitespace();
    let id = it.next()?.parse().ok()?;
    let src: u8 = it.next()?.parse().ok()?;
    let seq = it.next()?.parse().ok()?;
    if src as usize >= super::proto::MAX_SOURCES || it.next().is_some() {
        return None;
    }
    Some((id, src, seq, &m[nl + 1..]))
}

/// The state of player `id` as the page mirrors it, with the events to fire (in order).
pub fn state_message(id: u32, st: &State, position: i64, events: &[Event]) -> Vec<u8> {
    let mut s = String::with_capacity(160);
    let _ = write!(s, "{{\"t\":\"media\",\"id\":{id},\"ev\":[");
    for (i, e) in events.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(s, "\"{}\"", e.name());
    }
    let (w, h, v, a) = st.meta.map_or((0, 0, false, false), |m| {
        (m.width, m.height, m.video, m.audio)
    });
    let _ = write!(
        s,
        "],\"ct\":{},\"p\":{},\"e\":{},\"sk\":{},\"rs\":{},\"w\":{},\"h\":{},\"v\":{},\"a\":{},\"d\":{},\"vol\":{},\"mu\":{},\"err\":{}}}",
        position,
        u8::from(st.paused),
        u8::from(st.ended),
        u8::from(st.seeking.is_some()),
        st.ready,
        w,
        h,
        u8::from(v),
        u8::from(a),
        match (st.meta, st.duration) {
            (None, _) => -2,
            (_, None) => -1,
            (_, Some(d)) => d,
        },
        st.volume,
        u8::from(st.muted),
        st.error.unwrap_or(0)
    );
    s.into_bytes()
}

/// What source buffer `src` of player `id` holds after the append with sequence number `seq`
/// (`status`: `proto::BUF_*`).
pub fn buffered_message(id: u32, src: u8, seq: u32, status: u8, ranges: &[(i64, i64)]) -> Vec<u8> {
    let mut s = format!("{{\"t\":\"media\",\"id\":{id},\"buf\":[{src},{seq},{status},[");
    for (i, (a, b)) in ranges.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(s, "[{a},{b}]");
    }
    s.push_str("]]}");
    s.into_bytes()
}

/// Where the cluster for time `t` of source `src` is (`None`: the file has no index).
pub fn cue_message(id: u32, src: u8, t: i64, offset: Option<u64>) -> Vec<u8> {
    let o = offset.map_or(-1, |o| o.min(i64::MAX as u64) as i64);
    format!("{{\"t\":\"media\",\"id\":{id},\"cue\":[{src},{t},{o}]}}").into_bytes()
}

/// An error of player `id` that is not the decoder's (no free player, ...): `MediaError` `code`.
pub fn error_message(id: u32, code: u8, text: &str) -> Vec<u8> {
    let mut s = format!("{{\"t\":\"media\",\"id\":{id},\"fail\":[{code},");
    crate::browsing::web::script::json_str(&mut s, text);
    s.push_str("]}");
    s.into_bytes()
}
