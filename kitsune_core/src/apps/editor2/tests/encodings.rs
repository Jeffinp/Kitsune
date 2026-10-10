//! Opening and saving files that are not plain UTF-8.

use super::*;
use crate::format::text::Encoding;

#[test]
fn a_windows_1252_file_is_edited_as_text_and_saved_the_same_way() {
    let raw = b"a\xe7\xe3o \x96 ok\r\n";
    let mut e = Editor::new();
    e.load_file(raw);
    assert_eq!(e.encoding(), Encoding::Windows1252);
    assert_eq!(text(&e), "ação – ok\r\n");
    assert_eq!(e.eol(), Eol::Crlf);
    assert_eq!(e.status().encoding, Encoding::Windows1252);
    e.insert_str("é ");
    assert_eq!(e.file_bytes(), b"\xe9 a\xe7\xe3o \x96 ok\r\n");
    assert_eq!(e.encoding(), Encoding::Windows1252);
}

#[test]
fn typing_a_character_the_encoding_lacks_turns_the_file_into_utf8() {
    let mut e = Editor::new();
    e.load_file(b"caf\xe9");
    e.insert_str("日");
    assert_eq!(e.file_bytes(), "日café".as_bytes());
    assert_eq!(e.encoding(), Encoding::Utf8);
}

#[test]
fn utf16_and_bom_files_keep_their_form() {
    let utf16 = crate::format::text::encode("olá\n".as_bytes(), Encoding::Utf16Le).unwrap();
    let mut e = Editor::new();
    e.load_file(&utf16);
    assert_eq!(
        (text(&e).as_str(), e.encoding()),
        ("olá\n", Encoding::Utf16Le)
    );
    assert_eq!(e.file_bytes(), utf16);
    e.load_file(b"\xEF\xBB\xBFabc");
    assert_eq!(
        (text(&e).as_str(), e.encoding()),
        ("abc", Encoding::Utf8Bom)
    );
    assert_eq!(e.file_bytes(), b"\xEF\xBB\xBFabc");
}

#[test]
fn set_text_and_binary_files_stay_utf8() {
    let mut e = Editor::new();
    e.load_file(b"\x00\xff\xfe");
    assert_eq!(e.encoding(), Encoding::Utf8);
    assert_eq!(e.file_bytes(), b"\x00\xff\xfe");
    e.load_file(b"caf\xe9");
    e.set_text(b"plain");
    assert_eq!(e.encoding(), Encoding::Utf8);
}

#[test]
fn the_status_bar_names_the_encoding() {
    let mut e = Editor::new();
    e.load_file(b"caf\xe9");
    let bar = crate::apps::editor2::dialog::status_bar(&e.status());
    assert_eq!(bar.facts[0], "Windows-1252");
}
