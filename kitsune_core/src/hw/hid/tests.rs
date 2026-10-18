use super::*;

fn kb(modifiers: u8, keys: &[u8]) -> [u8; 8] {
    let mut r = [0u8; 8];
    r[0] = modifiers;
    r[2..2 + keys.len()].copy_from_slice(keys);
    r
}

fn run(k: &mut BootKeyboard, report: &[u8]) -> Vec<(u8, bool, bool)> {
    let mut out = Vec::new();
    k.report(report, |e| out.push((e.scan_code, e.extended, e.pressed)));
    out
}

// ---- usage → scancode ----

#[test]
fn letters_follow_the_physical_positions() {
    // HID 0x04 'a' is the key at the position of A: set 1 0x1E; 0x1D 'z' is 0x2C.
    assert_eq!(usage_to_scancode(0x04), Some((0x1E, false)));
    assert_eq!(usage_to_scancode(0x05), Some((0x30, false))); // b
    assert_eq!(usage_to_scancode(0x10), Some((0x32, false))); // m
    assert_eq!(usage_to_scancode(0x14), Some((0x10, false))); // q
    assert_eq!(usage_to_scancode(0x1D), Some((0x2C, false))); // z
}

#[test]
fn digits_and_punctuation() {
    assert_eq!(usage_to_scancode(0x1E), Some((0x02, false))); // 1
    assert_eq!(usage_to_scancode(0x26), Some((0x0A, false))); // 9
    assert_eq!(usage_to_scancode(0x27), Some((0x0B, false))); // 0
    assert_eq!(usage_to_scancode(0x2C), Some((0x39, false))); // space
    assert_eq!(usage_to_scancode(0x28), Some((0x1C, false))); // enter
    assert_eq!(usage_to_scancode(0x35), Some((0x29, false))); // `
    assert_eq!(usage_to_scancode(0x87), Some((0x73, false))); // ABNT2 / ?
}

#[test]
fn function_navigation_and_keypad_keys() {
    assert_eq!(usage_to_scancode(0x3A), Some((0x3B, false))); // F1
    assert_eq!(usage_to_scancode(0x43), Some((0x44, false))); // F10
    assert_eq!(usage_to_scancode(0x44), Some((0x57, false))); // F11
    assert_eq!(usage_to_scancode(0x45), Some((0x58, false))); // F12
    assert_eq!(usage_to_scancode(0x52), Some((0x48, true))); // up
    assert_eq!(usage_to_scancode(0x51), Some((0x50, true))); // down
    assert_eq!(usage_to_scancode(0x50), Some((0x4B, true))); // left
    assert_eq!(usage_to_scancode(0x4F), Some((0x4D, true))); // right
    assert_eq!(usage_to_scancode(0x4C), Some((0x53, true))); // delete
    assert_eq!(usage_to_scancode(0x4B), Some((0x49, true))); // page up
    assert_eq!(usage_to_scancode(0x4E), Some((0x51, true))); // page down
    assert_eq!(usage_to_scancode(0x58), Some((0x1C, true))); // keypad enter
    assert_eq!(usage_to_scancode(0x59), Some((0x4F, false))); // keypad 1
    assert_eq!(usage_to_scancode(0x61), Some((0x49, false))); // keypad 9
    assert_eq!(usage_to_scancode(0x62), Some((0x52, false))); // keypad 0
}

#[test]
fn unmapped_usages_have_no_key() {
    for usage in [0x00, 0x01, 0x02, 0x03, 0x48, 0x66, 0xE0, 0xFF] {
        assert_eq!(usage_to_scancode(usage), None, "usage {usage:#x}");
    }
}

#[test]
fn every_mapped_usage_maps_to_a_distinct_key_except_the_known_aliases() {
    let mut seen = std::collections::HashMap::new();
    for usage in 0..=255u8 {
        if let Some(key) = usage_to_scancode(usage)
            && let Some(other) = seen.insert(key, usage)
        {
            // 0x31 and 0x32 (backslash and the non-US #) share a key on purpose
            assert_eq!((other, usage), (0x31, 0x32), "{key:?}");
        }
    }
}

// ---- keyboard reports ----

#[test]
fn a_key_press_then_release() {
    let mut k = BootKeyboard::new();
    assert_eq!(run(&mut k, &kb(0, &[0x04])), vec![(0x1E, false, true)]);
    assert_eq!(
        run(&mut k, &kb(0, &[0x04])),
        vec![],
        "an unchanged report is silent"
    );
    assert_eq!(run(&mut k, &kb(0, &[])), vec![(0x1E, false, false)]);
}

#[test]
fn modifiers_come_before_the_key_they_modify_and_after_it_on_release() {
    let mut k = BootKeyboard::new();
    // left shift (bit 1) + 'a' arrive in one report
    assert_eq!(
        run(&mut k, &kb(0b10, &[0x04])),
        vec![(0x2A, false, true), (0x1E, false, true)]
    );
    assert_eq!(
        run(&mut k, &kb(0, &[])),
        vec![(0x1E, false, false), (0x2A, false, false)]
    );
}

#[test]
fn every_modifier_bit_has_its_scancode() {
    let expected = [
        (0x1D, false),
        (0x2A, false),
        (0x38, false),
        (0x5B, true),
        (0x1D, true),
        (0x36, false),
        (0x38, true),
        (0x5C, true),
    ];
    for (bit, &(scan, ext)) in expected.iter().enumerate() {
        let mut k = BootKeyboard::new();
        assert_eq!(
            run(&mut k, &kb(1 << bit, &[])),
            vec![(scan, ext, true)],
            "bit {bit}"
        );
        assert_eq!(
            run(&mut k, &kb(0, &[])),
            vec![(scan, ext, false)],
            "bit {bit}"
        );
    }
}

#[test]
fn several_keys_and_a_partial_release() {
    let mut k = BootKeyboard::new();
    assert_eq!(
        run(&mut k, &kb(0, &[0x04, 0x05])),
        vec![(0x1E, false, true), (0x30, false, true)]
    );
    assert_eq!(run(&mut k, &kb(0, &[0x05])), vec![(0x1E, false, false)]);
    // the slot order changing is not a press
    assert_eq!(
        run(&mut k, &kb(0, &[0x05, 0x06])),
        vec![(0x2E, false, true)]
    );
}

#[test]
fn the_same_usage_twice_in_a_report_is_one_press() {
    let mut k = BootKeyboard::new();
    assert_eq!(
        run(&mut k, &kb(0, &[0x04, 0x04])),
        vec![(0x1E, false, true)]
    );
    assert_eq!(run(&mut k, &kb(0, &[])), vec![(0x1E, false, false)]);
}

#[test]
fn rollover_error_and_short_reports_change_nothing() {
    let mut k = BootKeyboard::new();
    run(&mut k, &kb(0, &[0x04]));
    assert_eq!(run(&mut k, &kb(0, &[0x01; 6])), vec![]);
    assert_eq!(run(&mut k, &[0, 0, 0]), vec![]);
    // still holding 'a': releasing it works
    assert_eq!(run(&mut k, &kb(0, &[])), vec![(0x1E, false, false)]);
}

#[test]
fn a_report_longer_than_eight_bytes_uses_the_first_eight() {
    let mut k = BootKeyboard::new();
    let mut r = kb(0, &[0x04]).to_vec();
    r.extend([9, 9, 9]);
    assert_eq!(run(&mut k, &r), vec![(0x1E, false, true)]);
}

#[test]
fn unmapped_keys_are_tracked_but_silent() {
    let mut k = BootKeyboard::new();
    assert_eq!(run(&mut k, &kb(0, &[0x48])), vec![]); // Pause
    assert_eq!(
        run(&mut k, &kb(0, &[0x48, 0x04])),
        vec![(0x1E, false, true)]
    );
    assert_eq!(run(&mut k, &kb(0, &[])), vec![(0x1E, false, false)]);
}

#[test]
fn release_all_lets_go_of_everything() {
    let mut k = BootKeyboard::new();
    run(&mut k, &kb(0b101, &[0x04, 0x52]));
    let mut released = Vec::new();
    k.release_all(|e| released.push((e.scan_code, e.extended, e.pressed)));
    assert_eq!(released.len(), 4);
    assert!(released.iter().all(|&(_, _, pressed)| !pressed));
    let mut again = Vec::new();
    k.release_all(|e| again.push(e));
    assert!(again.is_empty());
}

/// Whatever the reports, the events must be balanced: never a press of a key already down, never a
/// release of one that is up, and `release_all` leaves nothing down.
#[test]
fn random_reports_keep_press_and_release_balanced() {
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    let mut k = BootKeyboard::new();
    let mut down = std::collections::HashSet::new();
    for _ in 0..20_000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let mut r = [0u8; 8];
        for (i, b) in r.iter_mut().enumerate() {
            // keys drawn from a small alphabet so presses and releases actually happen
            *b = if i == 0 {
                (x >> 56) as u8
            } else {
                ((x >> (i * 6)) as u8) % 12
            };
        }
        k.report(&r, |e| {
            let id = (e.scan_code, e.extended);
            if e.pressed {
                assert!(down.insert(id), "press of a key already down: {id:?}");
            } else {
                assert!(down.remove(&id), "release of a key that is up: {id:?}");
            }
        });
    }
    k.release_all(|e| {
        assert!(!e.pressed);
        down.remove(&(e.scan_code, e.extended));
    });
    assert!(down.is_empty());
}

// ---- mouse ----

#[test]
fn mouse_report_with_buttons_and_motion() {
    let p = mouse_packet(&[0b101, 5, 0xFD]).unwrap(); // left + middle, +5 right, -3 (up on screen)
    assert!(p.left && p.middle && !p.right);
    assert_eq!(p.dx, 5);
    assert_eq!(
        p.dy, 3,
        "HID Y grows downward; the desktop wants up-positive"
    );
    assert_eq!(p.dz, 0);
}

#[test]
fn mouse_wheel_direction_matches_ps2() {
    // HID wheel +1 is "away from the user" (scroll up); the desktop wants toward-the-user positive
    assert_eq!(mouse_packet(&[0, 0, 0, 1]).unwrap().dz, -1);
    assert_eq!(mouse_packet(&[0, 0, 0, 0xFF]).unwrap().dz, 1);
}

#[test]
fn mouse_extremes_stay_in_range() {
    let p = mouse_packet(&[2, 0x80, 0x80, 0x80]).unwrap();
    assert_eq!(p.dx, -128);
    assert_eq!(p.dy, 128);
    assert_eq!(p.dz, WHEEL_MAX, "-(-128) is clamped to the symmetric range");
    assert!(p.right);
}

#[test]
fn mouse_report_too_short() {
    assert_eq!(mouse_packet(&[]), None);
    assert_eq!(mouse_packet(&[1, 2]), None);
    assert!(mouse_packet(&[1, 2, 3]).is_some());
}

// ---- key repeat ----

fn press(scan: u8, ext: bool) -> KeyEvent {
    KeyEvent {
        scan_code: scan,
        pressed: true,
        extended: ext,
    }
}
fn release(scan: u8, ext: bool) -> KeyEvent {
    KeyEvent {
        scan_code: scan,
        pressed: false,
        extended: ext,
    }
}

#[test]
fn typematic_waits_for_the_delay_then_repeats_at_the_interval() {
    let mut t = Typematic::new(500, 33);
    t.on_key(press(0x1E, false), 1000);
    assert_eq!(t.poll(1000), None);
    assert_eq!(t.poll(1499), None);
    assert_eq!(t.poll(1500), Some(press(0x1E, false)));
    assert_eq!(t.poll(1520), None);
    assert_eq!(t.poll(1533), Some(press(0x1E, false)));
}

#[test]
fn typematic_stops_on_release_of_the_repeating_key() {
    let mut t = Typematic::new(500, 33);
    t.on_key(press(0x1E, false), 0);
    t.on_key(release(0x1E, false), 100);
    assert_eq!(t.poll(10_000), None);
}

#[test]
fn typematic_follows_the_last_key_pressed() {
    let mut t = Typematic::new(500, 33);
    t.on_key(press(0x1E, false), 0);
    t.on_key(press(0x30, false), 100);
    assert_eq!(t.poll(550), None, "the delay restarts for the new key");
    assert_eq!(t.poll(600), Some(press(0x30, false)));
    // releasing the OLD key does not stop the repeat of the new one
    t.on_key(release(0x1E, false), 610);
    assert_eq!(t.poll(640), Some(press(0x30, false)));
}

#[test]
fn typematic_ignores_modifiers_and_locks() {
    for (scan, ext) in [
        (0x2A, false),
        (0x1D, false),
        (0x38, true),
        (0x3A, false),
        (0x5B, true),
    ] {
        let mut t = Typematic::new(500, 33);
        t.on_key(press(scan, ext), 0);
        assert_eq!(t.poll(5000), None, "{scan:#x}/{ext}");
    }
}

#[test]
fn typematic_extended_and_plain_keys_are_different_keys() {
    let mut t = Typematic::new(500, 33);
    t.on_key(press(0x4B, true), 0); // left arrow
    t.on_key(release(0x4B, false), 10); // keypad 4: a different key
    assert_eq!(t.poll(600), Some(press(0x4B, true)));
}

#[test]
fn typematic_after_a_long_stall_gives_one_repeat_not_a_burst() {
    let mut t = Typematic::new(500, 33);
    t.on_key(press(0x1E, false), 0);
    assert!(t.poll(10_000).is_some());
    assert_eq!(t.poll(10_000), None);
    assert!(t.poll(10_033).is_some());
}

#[test]
fn typematic_with_a_zero_interval_cannot_spin() {
    let mut t = Typematic::new(0, 0);
    t.on_key(press(0x1E, false), 0);
    assert!(t.poll(0).is_some());
    assert_eq!(t.poll(0), None);
}

#[test]
fn two_usages_for_the_same_key_press_and_release_it_once() {
    // 0x31 (backslash) and 0x32 (non-US #) are the same key: a device holding both is one key down
    let mut k = BootKeyboard::new();
    assert_eq!(
        run(&mut k, &kb(0, &[0x31, 0x32])),
        vec![(0x2B, false, true)]
    );
    assert_eq!(
        run(&mut k, &kb(0, &[0x32])),
        vec![],
        "still held through the other usage"
    );
    assert_eq!(run(&mut k, &kb(0, &[])), vec![(0x2B, false, false)]);
}

/// The key repeat only ever repeats a key that is physically held, fed from random keyboard reports.
#[test]
fn random_reports_never_repeat_a_released_key() {
    let mut x = 0xD1B5_4A32_D192_ED03u64;
    let mut k = BootKeyboard::new();
    let mut t = Typematic::new(500, 33);
    let mut down = std::collections::HashSet::new();
    let mut now = 0u64;
    for _ in 0..20_000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        now += 1 + (x >> 40) % 400;
        let mut r = [0u8; 8];
        for (i, b) in r.iter_mut().enumerate() {
            *b = if i == 0 {
                (x >> 56) as u8
            } else {
                ((x >> (i * 6)) as u8) % 12
            };
        }
        let mut events = Vec::new();
        k.report(&r, |e| events.push(e));
        for e in events {
            if e.pressed {
                down.insert((e.scan_code, e.extended));
            } else {
                down.remove(&(e.scan_code, e.extended));
            }
            t.on_key(e, now);
        }
        if let Some(rep) = t.poll(now) {
            assert!(
                down.contains(&(rep.scan_code, rep.extended)),
                "repeated a key that is up"
            );
        }
    }
}
