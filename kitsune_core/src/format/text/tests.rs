use super::*;

#[test]
fn utf8_passes_through_and_boms_are_recognised() {
    assert_eq!(decode("olá".as_bytes()).encoding, Encoding::Utf8);
    let d = decode(b"\xEF\xBB\xBFabc");
    assert_eq!(
        (d.text.as_slice(), d.encoding),
        (&b"abc"[..], Encoding::Utf8Bom)
    );
    assert_eq!(
        encode(b"abc", Encoding::Utf8Bom).unwrap(),
        b"\xEF\xBB\xBFabc"
    );
    assert_eq!(decode(b"").encoding, Encoding::Utf8);
}

#[test]
fn latin1_and_windows1252_files_become_utf8() {
    // "ação – 50€ “ok”" in Windows-1252.
    let raw = b"a\xe7\xe3o \x96 50\x80 \x93ok\x94";
    let d = decode(raw);
    assert_eq!(d.encoding, Encoding::Windows1252);
    assert_eq!(
        String::from_utf8(d.text.clone()).unwrap(),
        "ação – 50€ “ok”"
    );
    assert_eq!(encode(&d.text, Encoding::Windows1252).unwrap(), raw);
    // A character outside the set cannot be written back that way.
    assert_eq!(encode("日本".as_bytes(), Encoding::Windows1252), None);
    // The five undefined bytes survive a round trip as controls.
    let odd = [0x81u8, 0x8d, 0x8f, 0x90, 0x9d, b'x'];
    let d = decode(&odd);
    assert_eq!(encode(&d.text, Encoding::Windows1252).unwrap(), odd);
}

#[test]
fn utf16_with_a_mark_round_trips() {
    let text = "Olá, 日本 😀\r\nfim";
    for enc in [Encoding::Utf16Le, Encoding::Utf16Be] {
        let raw = encode(text.as_bytes(), enc).unwrap();
        assert_eq!(
            &raw[..2],
            if enc == Encoding::Utf16Le {
                &[0xFF, 0xFE]
            } else {
                &[0xFE, 0xFF]
            }
        );
        let d = decode(&raw);
        assert_eq!(d.encoding, enc);
        assert_eq!(String::from_utf8(d.text).unwrap(), text);
    }
    // A lone surrogate and an odd trailing byte are damage, not a panic.
    let d = decode(&[0xFF, 0xFE, 0x00, 0xD8, b'a', 0x00, 0x99]);
    assert_eq!(d.encoding, Encoding::Utf16Le);
    assert!(String::from_utf8(d.text).unwrap().starts_with('\u{FFFD}'));
}

#[test]
fn binary_data_is_left_alone() {
    let blob = [0x00u8, 0xFF, 0xFE, 0x80, 0x00, 0x41];
    // Starts like UTF-16 LE by accident? No: 00 first.
    let d = decode(&blob);
    assert_eq!((d.text.as_slice(), d.encoding), (&blob[..], Encoding::Utf8));
    let blob = [b'A', 0, 0xC3, 0x28];
    assert_eq!(decode(&blob).text, blob);
}

#[test]
fn names_are_parsed_loosely() {
    for (n, e) in [
        ("UTF-8", Encoding::Utf8),
        ("utf8", Encoding::Utf8),
        ("utf-8-bom", Encoding::Utf8Bom),
        ("UTF-16LE", Encoding::Utf16Le),
        ("utf_16be", Encoding::Utf16Be),
        ("latin1", Encoding::Windows1252),
        ("ISO-8859-1", Encoding::Windows1252),
        ("CP1252", Encoding::Windows1252),
    ] {
        assert_eq!(Encoding::parse(n), Some(e), "{n}");
    }
    assert_eq!(Encoding::parse("ebcdic"), None);
}

#[test]
fn every_byte_value_decodes_and_encodes_without_panicking() {
    let all: Vec<u8> = (1..=255u8).collect();
    let d = decode(&all);
    assert_eq!(d.encoding, Encoding::Windows1252);
    assert_eq!(encode(&d.text, Encoding::Windows1252).unwrap(), all);
}

#[test]
fn decode_as_does_not_guess() {
    assert_eq!(
        decode_as(b"caf\xe9", Encoding::Windows1252),
        "café".as_bytes()
    );
    assert_eq!(
        decode_as(b"caf\xe9", Encoding::Utf8),
        "caf\u{FFFD}".as_bytes()
    );
    assert_eq!(decode_as(b"a\0b\0", Encoding::Utf16Le), b"ab");
    assert_eq!(decode_as(b"\xFE\xFF\0a", Encoding::Utf16Be), b"a");
    assert_eq!(decode_as(b"\xEF\xBB\xBFx", Encoding::Utf8Bom), b"x");
}
