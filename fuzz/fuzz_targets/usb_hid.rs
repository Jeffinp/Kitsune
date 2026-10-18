//! Fuzz target: what a USB device sends the kernel before it is trusted — descriptors and HID
//! reports (`kitsune_core::hw::{usb, hid}`).
//!
//! Input layout: `[mode, ...bytes]`; `mode & 3` picks the part under test.
//!
//! * `0`: device descriptor, its first 8 bytes, and the configuration header.
//! * `1`: the configuration blob walk (`boot_hid_interfaces`), into an output of 0 to 4 slots
//!   (`mode >> 2`). With `mode & 0x10` the bytes follow a valid header whose `wTotalLength` covers them,
//!   so the fuzzer starts inside the descriptors.
//! * `2`: boot keyboard reports, 8 bytes at a time, through `BootKeyboard` and `Typematic`.
//! * `3`: boot mouse reports of every length.
//!
//! Invariants: never a panic or an out-of-slice read; `boot_hid_interfaces` stores at most the output
//! length and only interfaces with an interrupt IN endpoint; keyboard events are balanced (no press of a
//! key already down, no release of one that is up) and `release_all` leaves nothing down; the key
//! repeat only ever repeats a key that is held; mouse deltas stay in range.
#![no_main]

use std::collections::HashSet;

use kitsune_core::hw::hid::{self, BootKeyboard, KEYBOARD_REPORT_LEN, Typematic};
use kitsune_core::hw::ps2::{KeyEvent, WHEEL_MAX};
use kitsune_core::hw::usb::{self, BootHid, BootProtocol, MAX_CONFIG_LEN};
use libfuzzer_sys::fuzz_target;

fn descriptors(bytes: &[u8]) {
    let _ = usb::parse_device(bytes);
    let _ = usb::max_packet_size0_of(bytes);
    if let Some(h) = usb::parse_config_header(bytes) {
        let n = usb::config_request_len(h.total_length);
        assert!((9..=MAX_CONFIG_LEN).contains(&n));
    }
}

fn configuration(mode: u8, bytes: &[u8]) {
    let blob = if mode & 0x10 != 0 {
        let total = 9 + bytes.len();
        let mut v = vec![9, 2, total as u8, (total >> 8) as u8, 1, 1, 0, 0xA0, 50];
        v.extend_from_slice(bytes);
        v
    } else {
        bytes.to_vec()
    };
    let blank = BootHid {
        configuration: 0,
        interface: 0,
        protocol: BootProtocol::Keyboard,
        endpoint: 0,
        max_packet: 0,
        interval: 0,
    };
    let mut out = vec![blank; usize::from((mode >> 2) & 3) + (mode as usize >> 5 & 1)];
    let n = usb::boot_hid_interfaces(&blob, &mut out);
    assert!(n <= out.len());
    for hid in &out[..n] {
        assert!(hid.endpoint & 0x80 != 0, "an endpoint kept must be IN");
        assert!(hid.max_packet <= 0x7FF);
    }
}

fn keyboard(bytes: &[u8]) {
    let mut kb = BootKeyboard::new();
    let mut repeat = Typematic::new(500, 33);
    let mut down: HashSet<(u8, bool)> = HashSet::new();
    let mut now = 0u64;
    let on_event = |e: KeyEvent, down: &mut HashSet<(u8, bool)>, repeat: &mut Typematic, now| {
        let id = (e.scan_code, e.extended);
        if e.pressed {
            assert!(down.insert(id), "press of a key already down: {id:?}");
        } else {
            assert!(down.remove(&id), "release of a key that is up: {id:?}");
        }
        repeat.on_key(e, now);
    };
    for chunk in bytes.chunks(KEYBOARD_REPORT_LEN) {
        now += 1 + u64::from(chunk[0]) * 7;
        let mut events = Vec::new();
        kb.report(chunk, |e| events.push(e));
        for e in events {
            on_event(e, &mut down, &mut repeat, now);
        }
        if let Some(r) = repeat.poll(now) {
            assert!(r.pressed);
            assert!(down.contains(&(r.scan_code, r.extended)), "repeat of a key not held");
        }
    }
    let mut events = Vec::new();
    kb.release_all(|e| events.push(e));
    for e in events {
        on_event(e, &mut down, &mut repeat, now);
    }
    assert!(down.is_empty(), "keys left down after release_all");
    assert!(repeat.poll(now + 100_000).is_none(), "repeat survived the release of every key");
}

fn mouse(bytes: &[u8]) {
    for len in 0..bytes.len().min(8) {
        if let Some(p) = hid::mouse_packet(&bytes[..=len]) {
            assert!((-128..=127).contains(&p.dx));
            assert!((-128..=128).contains(&p.dy));
            assert!((-WHEEL_MAX..=WHEEL_MAX).contains(&p.dz));
        } else {
            assert!(len < 2);
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, bytes)) = data.split_first() else {
        return;
    };
    match mode & 3 {
        0 => descriptors(bytes),
        1 => configuration(mode, bytes),
        2 => keyboard(bytes),
        _ => mouse(bytes),
    }
});
