//! Caret blink: the curve, the alpha and when it stops blinking.

/// Length of one caret blink, in milliseconds.
pub const BLINK_MS: u64 = 1060;

/// A caret keeps blinking this long after the last key or click, then rests solid (an idle
/// desktop must not repaint forever).
pub const BLINK_ACTIVE_MS: u64 = 12_000;

/// The caret stays solid this long after input.
pub const BLINK_HOLD_MS: u64 = 500;

pub(super) fn smoothstep256(x: u32) -> u32 {
    let x = x.min(256);
    (x * x * (768 - 2 * x)) >> 16
}

/// The caret opacity (0..=256) `ms` milliseconds into the blink, eased: solid, a quick fade
/// out, dark, a quick fade in.
pub fn caret_curve(ms: u64) -> u32 {
    let t = ms % BLINK_MS;
    match t {
        0..=419 => 256,
        420..=579 => 256 - smoothstep256(((t - 420) * 256 / 160) as u32),
        580..=899 => 0,
        _ => smoothstep256(((t - 900) * 256 / 160) as u32),
    }
}

/// The opacity of a caret whose owner saw input `since_ms` ago (`None`: never, so a fresh
/// window keeps a solid caret): solid for [`BLINK_HOLD_MS`], blinking until [`BLINK_ACTIVE_MS`],
/// solid afterwards.
pub fn caret_alpha(since_ms: Option<u64>) -> u32 {
    match since_ms {
        Some(s) if (BLINK_HOLD_MS..BLINK_ACTIVE_MS).contains(&s) => caret_curve(s - BLINK_HOLD_MS),
        _ => 256,
    }
}

/// Whether such a caret still changes (the window needs frames).
pub fn caret_animating(since_ms: Option<u64>) -> bool {
    since_ms.is_some_and(|s| s < BLINK_ACTIVE_MS)
}
