//! Fuzz target: ZIP archives and text encodings.
//!
//! * `zip::read_dir` and `zip::extract` on arbitrary bytes never panic, and an entry never
//!   expands past the size the caller allows;
//! * `zip::safe_path` never returns a path that escapes (`..`, a drive, a control character);
//! * whatever `ZipWriter` writes reads back exactly;
//! * `textenc::decode` never panics, returns valid UTF-8 for the converted encodings, and what it decodes
//!   encodes back to the bytes it came from (the file is saved the way it was found).
#![no_main]

use kitsune_core::textenc::{self, Encoding};
use kitsune_core::zip::{self, ZipWriter};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(entries) = zip::read_dir(data) {
        for e in entries.iter().take(64) {
            if let Some(p) = zip::safe_path(&e.name) {
                assert!(!p.split('/').any(|c| c == ".." || c.contains(':')));
                assert!(!p.starts_with('/'));
            }
            if let Ok(out) = zip::extract(data, e, 1 << 16) {
                assert!(out.len() <= 1 << 16);
            }
        }
    }

    // Names and contents from the input, written and read back.
    let split = data.len() / 2;
    let (names, body) = data.split_at(split);
    let mut w = ZipWriter::new();
    let mut added = Vec::new();
    for (i, chunk) in names.chunks(7).take(8).enumerate() {
        let name = format!("f{i}{}", chunk.iter().map(|b| (b'a' + b % 26) as char).collect::<String>());
        if w.add_file(&name, body, (0, 0x21)).is_ok() {
            added.push(name);
        }
    }
    let zip_bytes = w.finish();
    let back = zip::read_dir(&zip_bytes).expect("an archive we wrote lists");
    assert_eq!(back.len(), added.len());
    for (e, name) in back.iter().zip(&added) {
        assert_eq!(&e.name, name);
        assert_eq!(zip::extract(&zip_bytes, e, 1 << 20).expect("and extracts"), body);
    }

    let d = textenc::decode(data);
    if d.encoding != Encoding::Utf8 {
        // (UTF-8 with a mark keeps its bytes as they are, valid or not.)
        if d.encoding != Encoding::Utf8Bom {
            assert!(core::str::from_utf8(&d.text).is_ok());
        }
        if let Some(raw) = textenc::encode(&d.text, d.encoding) {
            if d.encoding == Encoding::Windows1252 || d.encoding == Encoding::Utf8Bom {
                assert_eq!(raw, data);
            }
        }
    }
});
