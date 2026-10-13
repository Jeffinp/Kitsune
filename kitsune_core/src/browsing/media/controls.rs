//! The controls the browser draws on a `<video controls>` (and the bar of an `<audio controls>`):
//! where each part sits in the element's box, what a click there means, and the time as text.
//! Pixels are drawn by the kernel; this is the geometry, so clicks and paint agree and it is tested.

use alloc::string::String;
use core::fmt::Write as _;

/// Height of the bar at zoom 100 %.
pub const BAR_H: i32 = 36;
/// An `<audio controls>` box at zoom 100 % (the size browsers give it).
pub const AUDIO_W: i32 = 300;
pub const AUDIO_H: i32 = 40;

/// A part of the controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// Play / pause.
    Play,
    /// The time bar: a click seeks there.
    Seek,
    /// Mute / unmute.
    Mute,
    /// The volume slider: a click sets the volume.
    Volume,
    /// The rest of the bar (no action).
    Bar,
}

/// A rectangle relative to the element's box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Area {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// Where everything is for a box of `w` x `h` at `zoom` percent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub bar: Area,
    pub play: Area,
    /// Where the time text goes (empty when the box is too narrow).
    pub time: Area,
    /// The time bar's track (the thin line is drawn centred in it).
    pub seek: Area,
    pub mute: Area,
    /// The volume slider (empty when the box is too narrow).
    pub volume: Area,
}

/// The controls of a box `w` x `h` at `zoom` percent.
pub fn layout(w: i32, h: i32, zoom: i32) -> Layout {
    let z = |v: i32| (v * zoom.max(10) / 100).max(1);
    let bh = z(BAR_H).min(h.max(1));
    let bar = Area {
        x: 0,
        y: h - bh,
        w,
        h: bh,
    };
    let pad = z(6);
    let btn = bh.min(z(32));
    let by = bar.y + (bh - btn) / 2;
    let play = Area {
        x: pad,
        y: by,
        w: btn,
        h: btn,
    };
    let mut right = w - pad;
    let volume = if w >= z(320) {
        let vw = z(64);
        right -= vw;
        Area {
            x: right,
            y: by,
            w: vw,
            h: btn,
        }
    } else {
        Area::default()
    };
    if volume.w > 0 {
        right -= z(4);
    }
    let mute = Area {
        x: right - btn,
        y: by,
        w: btn,
        h: btn,
    };
    right = mute.x - z(6);
    let mut left = play.x + play.w + z(6);
    let time = if w >= z(240) {
        let tw = z(84);
        let a = Area {
            x: left,
            y: by,
            w: tw,
            h: btn,
        };
        left += tw + z(4);
        a
    } else {
        Area::default()
    };
    let seek = Area {
        x: left,
        y: by,
        w: (right - left).max(0),
        h: btn,
    };
    Layout {
        bar,
        play,
        time,
        seek,
        mute,
        volume,
    }
}

impl Layout {
    /// The part under `(x, y)` (relative to the box), if the point is on the bar.
    pub fn hit(&self, x: i32, y: i32) -> Option<Part> {
        if !self.bar.contains(x, y) {
            return None;
        }
        Some(if self.play.contains(x, y) {
            Part::Play
        } else if self.seek.w > 0 && self.seek.contains(x, y) {
            Part::Seek
        } else if self.mute.contains(x, y) {
            Part::Mute
        } else if self.volume.w > 0 && self.volume.contains(x, y) {
            Part::Volume
        } else {
            Part::Bar
        })
    }

    /// The time a click at `x` on the time bar asks for, in a media of `duration` ns.
    pub fn seek_time(&self, x: i32, duration: i64) -> i64 {
        if self.seek.w <= 0 || duration <= 0 {
            return 0;
        }
        let f = i64::from((x - self.seek.x).clamp(0, self.seek.w));
        (duration / i64::from(self.seek.w)) * f
            + (duration % i64::from(self.seek.w)) * f / i64::from(self.seek.w)
    }

    /// The volume (0–100) a click at `x` on the slider asks for.
    pub fn volume_at(&self, x: i32) -> u8 {
        if self.volume.w <= 0 {
            return 100;
        }
        ((x - self.volume.x).clamp(0, self.volume.w) * 100 / self.volume.w) as u8
    }

    /// How far along the time bar `t` of `duration` is, in pixels.
    pub fn progress_px(&self, t: i64, duration: i64) -> i32 {
        if duration <= 0 || self.seek.w <= 0 {
            return 0;
        }
        let t = t.clamp(0, duration);
        ((t as i128 * i128::from(self.seek.w)) / i128::from(duration)) as i32
    }
}

/// A time as the controls show it: `m:ss`, or `h:mm:ss` from an hour on.
pub fn fmt_time(ns: i64) -> String {
    let s = (ns.max(0) / 1_000_000_000) as u64;
    let mut out = String::new();
    if s >= 3600 {
        let _ = write!(out, "{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60);
    } else {
        let _ = write!(out, "{}:{:02}", s / 60, s % 60);
    }
    out
}

/// Where a picture of `vw` x `vh` goes in a box of `bw` x `bh` (`object-fit: contain`, the
/// default of `<video>`): the largest size with the picture's shape, centred. Relative to the box.
pub fn contain(bw: i32, bh: i32, vw: u32, vh: u32) -> Area {
    if bw <= 0 || bh <= 0 || vw == 0 || vh == 0 {
        return Area {
            x: 0,
            y: 0,
            w: bw.max(0),
            h: bh.max(0),
        };
    }
    let (bw64, bh64, vw64, vh64) = (i64::from(bw), i64::from(bh), i64::from(vw), i64::from(vh));
    let (w, h) = if bw64 * vh64 > bh64 * vw64 {
        // The box is wider than the picture: bars at the sides.
        (((bh64 * vw64) / vh64).max(1), bh64)
    } else {
        (bw64, ((bw64 * vh64) / vw64).max(1))
    };
    Area {
        x: ((bw64 - w) / 2) as i32,
        y: ((bh64 - h) / 2) as i32,
        w: w as i32,
        h: h as i32,
    }
}

/// `current / duration` (the duration as `--:--` when unknown).
pub fn time_text(t: i64, duration: Option<i64>) -> String {
    let mut s = fmt_time(t);
    s.push_str(" / ");
    match duration {
        Some(d) => s.push_str(&fmt_time(d)),
        None => s.push_str("--:--"),
    }
    s
}
