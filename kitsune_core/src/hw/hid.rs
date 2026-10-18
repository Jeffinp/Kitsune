//! HID boot-protocol reports: keyboard (8 bytes) and mouse (3 or 4 bytes) turned into the same
//! [`KeyEvent`]s and [`Packet`]s the PS/2 decoder produces, so the desktop does not know which kind of
//! device it is talking to. Plus the software key repeat a USB keyboard needs (PS/2 hardware repeats; a
//! USB keyboard in boot protocol reports only changes).
//!
//! Keys are mapped to the position-based scancode set 1 the [`crate::system::keymap`] already reads,
//! so layouts (US, ABNT2) keep working: HID usage 0x04 is the key at the position of "A", whatever the
//! layout prints on it.

use super::ps2::{KeyEvent, Packet, WHEEL_MAX};

/// Bytes of a boot keyboard report: modifiers, reserved, six key usages.
pub const KEYBOARD_REPORT_LEN: usize = 8;

/// The scancode (set 1) and `extended` flag of a HID keyboard usage (usage page 0x07), or `None` for a
/// usage the desktop has no key for (Pause, media keys, international keys other than ABNT2's).
pub const fn usage_to_scancode(usage: u8) -> Option<(u8, bool)> {
    const LETTERS: [u8; 26] = [
        0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18,
        0x19, 0x10, 0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,
    ];
    // usages 0x59..=0x61 are keypad 1..9, in the order of the pad's rows from the bottom up
    const KEYPAD: [u8; 9] = [0x4F, 0x50, 0x51, 0x4B, 0x4C, 0x4D, 0x47, 0x48, 0x49];
    Some(match usage {
        0x04..=0x1D => (LETTERS[(usage - 0x04) as usize], false),
        0x1E..=0x26 => (0x02 + (usage - 0x1E), false), // 1..9
        0x27 => (0x0B, false),                         // 0
        0x28 => (0x1C, false),                         // Enter
        0x29 => (0x01, false),                         // Esc
        0x2A => (0x0E, false),                         // Backspace
        0x2B => (0x0F, false),                         // Tab
        0x2C => (0x39, false),                         // Space
        0x2D => (0x0C, false),                         // - _
        0x2E => (0x0D, false),                         // = +
        0x2F => (0x1A, false),                         // [ {
        0x30 => (0x1B, false),                         // ] }
        0x31 | 0x32 => (0x2B, false),                  // \ | (and the non-US # on ISO boards)
        0x33 => (0x27, false),                         // ; :
        0x34 => (0x28, false),                         // ' "
        0x35 => (0x29, false),                         // ` ~
        0x36 => (0x33, false),                         // , <
        0x37 => (0x34, false),                         // . >
        0x38 => (0x35, false),                         // / ?
        0x39 => (0x3A, false),                         // Caps Lock
        0x3A..=0x43 => (0x3B + (usage - 0x3A), false), // F1..F10
        0x44 => (0x57, false),                         // F11
        0x45 => (0x58, false),                         // F12
        0x46 => (0x37, true),                          // Print Screen
        0x47 => (0x46, false),                         // Scroll Lock
        0x49 => (0x52, true),                          // Insert
        0x4A => (0x47, true),                          // Home
        0x4B => (0x49, true),                          // Page Up
        0x4C => (0x53, true),                          // Delete
        0x4D => (0x4F, true),                          // End
        0x4E => (0x51, true),                          // Page Down
        0x4F => (0x4D, true),                          // Right
        0x50 => (0x4B, true),                          // Left
        0x51 => (0x50, true),                          // Down
        0x52 => (0x48, true),                          // Up
        0x53 => (0x45, false),                         // Num Lock
        0x54 => (0x35, true),                          // keypad /
        0x55 => (0x37, false),                         // keypad *
        0x56 => (0x4A, false),                         // keypad -
        0x57 => (0x4E, false),                         // keypad +
        0x58 => (0x1C, true),                          // keypad Enter
        0x59..=0x61 => (KEYPAD[(usage - 0x59) as usize], false),
        0x62 => (0x52, false), // keypad 0
        0x63 => (0x53, false), // keypad .
        0x64 => (0x56, false), // the extra key of ISO boards (< >)
        0x65 => (0x5D, true),  // Application (menu)
        0x85 => (0x7E, false), // keypad comma (ABNT2)
        0x87 => (0x73, false), // International 1: the ABNT2 / ? key
        _ => return None,
    })
}

/// Modifier bit (boot report byte 0) → scancode, in bit order: LCtrl, LShift, LAlt, LGUI, RCtrl, RShift,
/// RAlt, RGUI.
const MODIFIERS: [(u8, bool); 8] = [
    (0x1D, false),
    (0x2A, false),
    (0x38, false),
    (0x5B, true),
    (0x1D, true),
    (0x36, false),
    (0x38, true),
    (0x5C, true),
];

/// Some usage in `usages` (0 = empty slot) is the key `key`.
fn names_key(usages: &[u8], key: (u8, bool)) -> bool {
    usages.iter().any(|&u| usage_to_scancode(u) == Some(key))
}

/// The usage in a key slot that means "too many keys held": the whole report is to be ignored.
const ERROR_ROLL_OVER: u8 = 0x01;

/// Remembers the last keyboard report so the next one can be turned into press and release events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BootKeyboard {
    modifiers: u8,
    keys: [u8; 6],
}

impl BootKeyboard {
    pub const fn new() -> Self {
        Self {
            modifiers: 0,
            keys: [0; 6],
        }
    }

    /// Feeds one report and calls `emit` for each key that changed, in an order that keeps shortcuts
    /// right: key releases, modifier releases, modifier presses, key presses. A report shorter than
    /// [`KEYBOARD_REPORT_LEN`], or one that signals rollover, changes nothing. Usages without a mapping
    /// (see [`usage_to_scancode`]) are tracked but produce no event.
    pub fn report(&mut self, report: &[u8], mut emit: impl FnMut(KeyEvent)) {
        if report.len() < KEYBOARD_REPORT_LEN {
            return;
        }
        let modifiers = report[0];
        let mut keys = [0u8; 6];
        keys.copy_from_slice(&report[2..8]);
        if keys.contains(&ERROR_ROLL_OVER) {
            return;
        }

        // Two usages can name the same key (backslash and the non-US #), and a hostile device can
        // hold both: compare by key, not by usage, so presses and releases stay balanced.
        for (i, &usage) in self.keys.iter().enumerate() {
            if let Some(key) = usage_to_scancode(usage)
                && !names_key(&keys, key)
                && !names_key(&self.keys[..i], key)
            {
                emit(KeyEvent {
                    scan_code: key.0,
                    pressed: false,
                    extended: key.1,
                });
            }
        }
        let changed = self.modifiers ^ modifiers;
        for (bit, &(scan_code, extended)) in MODIFIERS.iter().enumerate() {
            if changed & (1 << bit) != 0 && modifiers & (1 << bit) == 0 {
                emit(KeyEvent {
                    scan_code,
                    pressed: false,
                    extended,
                });
            }
        }
        for (bit, &(scan_code, extended)) in MODIFIERS.iter().enumerate() {
            if changed & (1 << bit) != 0 && modifiers & (1 << bit) != 0 {
                emit(KeyEvent {
                    scan_code,
                    pressed: true,
                    extended,
                });
            }
        }
        for (i, &usage) in keys.iter().enumerate() {
            if let Some(key) = usage_to_scancode(usage)
                && !names_key(&self.keys, key)
                && !names_key(&keys[..i], key)
            {
                emit(KeyEvent {
                    scan_code: key.0,
                    pressed: true,
                    extended: key.1,
                });
            }
        }
        self.modifiers = modifiers;
        self.keys = keys;
    }

    /// Releases everything held (the keyboard was unplugged or reset), so no key stays stuck.
    pub fn release_all(&mut self, emit: impl FnMut(KeyEvent)) {
        self.report(&[0; KEYBOARD_REPORT_LEN], emit);
    }
}

/// A boot mouse report as the desktop's [`Packet`]. `report` is 3 bytes (buttons, X, Y) or 4 (plus the
/// wheel); shorter reports give `None`. HID reports Y growing downward and the wheel positive away from
/// the user; the PS/2 convention the desktop uses is the opposite for both, so they are negated.
pub fn mouse_packet(report: &[u8]) -> Option<Packet> {
    if report.len() < 3 {
        return None;
    }
    let wheel = report.get(3).map_or(0, |&b| b as i8 as i32);
    Some(Packet {
        dx: report[1] as i8 as i32,
        dy: -(report[2] as i8 as i32),
        dz: (-wheel).clamp(-WHEEL_MAX, WHEEL_MAX),
        left: report[0] & 0b001 != 0,
        right: report[0] & 0b010 != 0,
        middle: report[0] & 0b100 != 0,
    })
}

/// Software key repeat: after `delay_ms` held, the last key pressed repeats every `interval_ms`.
/// Modifiers and lock keys do not repeat, and pressing another key moves the repeat to it (as PS/2
/// hardware does). Release the key that is repeating to stop it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Typematic {
    delay_ms: u64,
    interval_ms: u64,
    held: Option<(u8, bool)>,
    next_ms: u64,
}

impl Typematic {
    pub const fn new(delay_ms: u64, interval_ms: u64) -> Self {
        Self {
            delay_ms,
            interval_ms,
            held: None,
            next_ms: 0,
        }
    }

    /// Call for every key event (the real ones, not the repeats).
    pub fn on_key(&mut self, key: KeyEvent, now_ms: u64) {
        let id = (key.scan_code, key.extended);
        if key.pressed {
            if repeats(key) {
                self.held = Some(id);
                self.next_ms = now_ms.saturating_add(self.delay_ms);
            }
        } else if self.held == Some(id) {
            self.held = None;
        }
    }

    /// A repeat press due at `now_ms`, if any. At most one per call, re-timed from `now_ms`, so a stall
    /// (the system was busy) does not turn into a burst of keystrokes.
    pub fn poll(&mut self, now_ms: u64) -> Option<KeyEvent> {
        let (scan_code, extended) = self.held?;
        if now_ms < self.next_ms {
            return None;
        }
        self.next_ms = now_ms.saturating_add(self.interval_ms.max(1));
        Some(KeyEvent {
            scan_code,
            pressed: true,
            extended,
        })
    }
}

/// Keys that auto-repeat: everything except modifiers, Windows keys and the lock keys.
const fn repeats(key: KeyEvent) -> bool {
    !matches!(
        (key.scan_code, key.extended),
        (0x1D | 0x2A | 0x36 | 0x38 | 0x3A | 0x45 | 0x46, false) | (0x1D | 0x38 | 0x5B | 0x5C, true)
    )
}

#[cfg(test)]
mod tests;
