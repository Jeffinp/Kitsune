//! EBML, the binary format under Matroska and WebM: variable-length integers, element headers and
//! the typed values (unsigned, signed, float, string, binary) read from a slice.

use super::Error;

/// The size of an element whose size field is all ones: "unknown" (it ends where its parent does or
/// where an element that cannot be its child begins; live streams and DASH segments use it).
pub(super) const UNKNOWN: u64 = u64::MAX;

/// An element header: its id (with the length marker bits, as the specification writes ids), the
/// size of its data ([`UNKNOWN`] for an unknown size) and how many bytes the header took.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Header {
    pub id: u32,
    pub size: u64,
    pub len: usize,
}

/// A variable-length integer at the start of `b`: `(value with the marker removed, length)`.
/// `Ok(None)` when `b` is too short to tell; an error for a first byte of zero (a length over 8).
pub(super) fn vint(b: &[u8]) -> Result<Option<(u64, usize)>, Error> {
    let Some(&first) = b.first() else {
        return Ok(None);
    };
    if first == 0 {
        return Err(Error::Invalid("variable-length integer longer than 8 bytes"));
    }
    let len = first.leading_zeros() as usize + 1;
    if b.len() < len {
        return Ok(None);
    }
    let mut v = u64::from(first) & (0xFF >> len);
    for &x in &b[1..len] {
        v = (v << 8) | u64::from(x);
    }
    Ok(Some((v, len)))
}

/// The element header at the start of `b`; `Ok(None)` when more bytes are needed.
pub(super) fn header(b: &[u8]) -> Result<Option<Header>, Error> {
    let Some(&first) = b.first() else {
        return Ok(None);
    };
    // Ids are 1 to 4 bytes long and keep their marker bits.
    let id_len = first.leading_zeros() as usize + 1;
    if first == 0 || id_len > 4 {
        return Err(Error::Invalid("element id longer than 4 bytes"));
    }
    if b.len() < id_len {
        return Ok(None);
    }
    let id = b[..id_len]
        .iter()
        .fold(0u32, |acc, &x| (acc << 8) | u32::from(x));
    let Some((raw, size_len)) = vint(&b[id_len..])? else {
        return Ok(None);
    };
    // All value bits set means "unknown size".
    let all_ones = (1u64 << (7 * size_len)) - 1;
    let size = if raw == all_ones { UNKNOWN } else { raw };
    Ok(Some(Header {
        id,
        size,
        len: id_len + size_len,
    }))
}

/// A cursor over the children of a master element that is entirely in memory (Info, Tracks, Cues,
/// a BlockGroup...). Children with an unknown size or that run past the end are errors.
pub(super) struct Children<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Children<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
}

impl<'a> Iterator for Children<'a> {
    type Item = Result<(u32, &'a [u8]), Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.data.get(self.pos..)?;
        if rest.is_empty() {
            return None;
        }
        let h = match header(rest) {
            Ok(Some(h)) => h,
            Ok(None) => {
                self.pos = self.data.len();
                return Some(Err(Error::Invalid("element header cut short")));
            }
            Err(e) => {
                self.pos = self.data.len();
                return Some(Err(e));
            }
        };
        let end = usize::try_from(h.size)
            .ok()
            .and_then(|s| h.len.checked_add(s))
            .filter(|&e| e <= rest.len());
        let Some(end) = end else {
            self.pos = self.data.len();
            return Some(Err(Error::Invalid("element larger than its parent")));
        };
        self.pos += end;
        Some(Ok((h.id, &rest[h.len..end])))
    }
}

/// An unsigned integer element (0 to 8 bytes, big endian).
pub(super) fn uint(b: &[u8]) -> Result<u64, Error> {
    if b.len() > 8 {
        return Err(Error::Invalid("integer longer than 8 bytes"));
    }
    Ok(b.iter().fold(0u64, |acc, &x| (acc << 8) | u64::from(x)))
}

/// A signed integer element (0 to 8 bytes, big endian, two's complement).
pub(super) fn int(b: &[u8]) -> Result<i64, Error> {
    let u = uint(b)?;
    if b.is_empty() {
        return Ok(0);
    }
    let shift = 64 - 8 * b.len() as u32;
    // Sign-extend from the element's width.
    Ok(((u << shift) as i64) >> shift)
}

/// A float element (0, 4 or 8 bytes).
pub(super) fn float(b: &[u8]) -> Result<f64, Error> {
    match b.len() {
        0 => Ok(0.0),
        4 => Ok(f64::from(f32::from_bits(uint(b)? as u32))),
        8 => Ok(f64::from_bits(uint(b)?)),
        _ => Err(Error::Invalid("float of a size other than 4 or 8")),
    }
}

/// A string element: ASCII or UTF-8, possibly padded with NULs, which are dropped.
pub(super) fn string(b: &[u8]) -> &[u8] {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    &b[..end]
}
