//! Text encodings: UTF-8 (with or without a byte-order mark), UTF-16 (LE and BE) and
//! Windows-1252 (which also reads Latin-1 text, apart from the 0x80-0x9F range).
//!
//! [`decode`] guesses the encoding of a file's bytes and returns UTF-8 plus what it found, so an
//! editor can show the text right and [`encode`] can write it back the way it came. Binary data
//! (it holds a NUL and is not UTF-16) is never "converted": it comes back untouched as UTF-8, with
//! invalid bytes left for the caller to show as it always did.
//!
//! Nothing here panics on any input.

use alloc::string::String;
use alloc::vec::Vec;

/// How a file's bytes spell its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Encoding {
    #[default]
    Utf8,
    /// UTF-8 with the `EF BB BF` mark at the start.
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Windows1252,
}

impl Encoding {
    /// The name shown in a status bar.
    pub const fn label(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf8Bom => "UTF-8 BOM",
            Encoding::Utf16Le => "UTF-16 LE",
            Encoding::Utf16Be => "UTF-16 BE",
            Encoding::Windows1252 => "Windows-1252",
        }
    }

    /// An encoding by the name people give it (`utf8`, `UTF-16LE`, `latin1`, `cp1252`...).
    /// `utf8-bom` and the like are accepted too.
    pub fn parse(name: &str) -> Option<Encoding> {
        let n: String = name
            .chars()
            .filter(|c| !matches!(c, '-' | '_' | ' '))
            .map(|c| c.to_ascii_lowercase())
            .collect();
        Some(match n.as_str() {
            "utf8" => Encoding::Utf8,
            "utf8bom" => Encoding::Utf8Bom,
            "utf16le" | "utf16" => Encoding::Utf16Le,
            "utf16be" => Encoding::Utf16Be,
            "windows1252" | "cp1252" | "latin1" | "iso88591" | "iso885915" | "ansi" => {
                Encoding::Windows1252
            }
            _ => return None,
        })
    }
}

/// UTF-8 text and the encoding it was read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub text: Vec<u8>,
    pub encoding: Encoding,
}

/// The characters of Windows-1252 at 0x80..=0x9F. The five bytes it leaves undefined map to the
/// control character of the same number, as browsers do, so they survive a round trip.
const CP1252_HIGH: [char; 32] = [
    '\u{20ac}', '\u{81}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8d}', '\u{17d}', '\u{8f}',
    '\u{90}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{9d}', '\u{17e}', '\u{178}',
];

/// Windows-1252 bytes as text.
pub fn windows1252_to_string(b: &[u8]) -> String {
    b.iter()
        .map(|&x| match x {
            0x80..=0x9f => CP1252_HIGH[usize::from(x - 0x80)],
            _ => char::from(x),
        })
        .collect()
}

fn to_windows1252(c: char) -> Option<u8> {
    let cp = c as u32;
    if cp < 0x80 || (0xA0..=0xFF).contains(&cp) {
        return Some(cp as u8);
    }
    CP1252_HIGH
        .iter()
        .position(|&h| h == c)
        .map(|i| 0x80 + i as u8)
}

fn utf16_to_string(b: &[u8], big: bool) -> String {
    let units = b.as_chunks::<2>().0.iter().map(|p| {
        if big {
            u16::from_be_bytes([p[0], p[1]])
        } else {
            u16::from_le_bytes([p[0], p[1]])
        }
    });
    char::decode_utf16(units)
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// Read `data` as text. See the module notes for what is guessed.
pub fn decode(data: &[u8]) -> Decoded {
    let utf8 = |text: &[u8], encoding| Decoded {
        text: text.to_vec(),
        encoding,
    };
    if let Some(rest) = data.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return utf8(rest, Encoding::Utf8Bom);
    }
    if let Some(rest) = data.strip_prefix(&[0xFF, 0xFE]) {
        return Decoded {
            text: utf16_to_string(rest, false).into_bytes(),
            encoding: Encoding::Utf16Le,
        };
    }
    if let Some(rest) = data.strip_prefix(&[0xFE, 0xFF]) {
        return Decoded {
            text: utf16_to_string(rest, true).into_bytes(),
            encoding: Encoding::Utf16Be,
        };
    }
    if core::str::from_utf8(data).is_ok() || data.contains(&0) {
        return utf8(data, Encoding::Utf8);
    }
    // Not UTF-8 and not binary: the usual old Windows text file.
    Decoded {
        text: windows1252_to_string(data).into_bytes(),
        encoding: Encoding::Windows1252,
    }
}

/// Read `data` as `enc` (no guessing), leaving UTF-8. A byte-order mark that matches is dropped;
/// what `enc` cannot spell becomes U+FFFD.
pub fn decode_as(data: &[u8], enc: Encoding) -> Vec<u8> {
    match enc {
        Encoding::Utf8 => String::from_utf8_lossy(data).into_owned().into_bytes(),
        Encoding::Utf8Bom => {
            let d = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
            String::from_utf8_lossy(d).into_owned().into_bytes()
        }
        Encoding::Utf16Le => {
            let d = data.strip_prefix(&[0xFF, 0xFE]).unwrap_or(data);
            utf16_to_string(d, false).into_bytes()
        }
        Encoding::Utf16Be => {
            let d = data.strip_prefix(&[0xFE, 0xFF]).unwrap_or(data);
            utf16_to_string(d, true).into_bytes()
        }
        Encoding::Windows1252 => windows1252_to_string(data).into_bytes(),
    }
}

/// Write UTF-8 `text` in `enc`. `None` when it cannot be represented (a character outside
/// Windows-1252, or `text` that is not valid UTF-8 where the encoding needs characters): the
/// caller then keeps UTF-8.
pub fn encode(text: &[u8], enc: Encoding) -> Option<Vec<u8>> {
    match enc {
        Encoding::Utf8 => Some(text.to_vec()),
        Encoding::Utf8Bom => {
            let mut out = Vec::with_capacity(text.len() + 3);
            out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            out.extend_from_slice(text);
            Some(out)
        }
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let s = core::str::from_utf8(text).ok()?;
            let big = enc == Encoding::Utf16Be;
            let mut out = Vec::with_capacity(2 + s.len() * 2);
            out.extend_from_slice(if big { &[0xFE, 0xFF] } else { &[0xFF, 0xFE] });
            for u in s.encode_utf16() {
                out.extend_from_slice(&if big {
                    u.to_be_bytes()
                } else {
                    u.to_le_bytes()
                });
            }
            Some(out)
        }
        Encoding::Windows1252 => {
            let s = core::str::from_utf8(text).ok()?;
            s.chars().map(to_windows1252).collect()
        }
    }
}

#[cfg(test)]
mod tests;
