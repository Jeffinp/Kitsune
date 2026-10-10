//! `Content-Encoding` support for the browser: gzip (RFC 1952), zlib-wrapped
//! `deflate` (RFC 1950, what the HTTP spec says) and bare `deflate` (what some
//! servers actually send), all on top of [`crate::format::inflate`] with a hard output
//! limit so a compression bomb cannot exhaust the kernel heap.

use crate::format::inflate::{self, InflateError};
use alloc::vec::Vec;

/// Largest decompressed body accepted. 4x the raw response cap
/// ([`crate::browsing::browser::MAX_RESPONSE_BYTES`], 1 MiB): HTML routinely compresses
/// 4-8x, so a page that fills the raw cap with gzip needs this much room. Memory
/// while decoding: the raw response + its de-chunked copy + this output (+ the
/// `Vec` growth slack, at most 2x) ~ 12 MiB worst case of the 64 MiB heap, freed
/// as soon as the DOM is built. Beyond it the decoded prefix is still shown.
pub const MAX_DECODED_BYTES: usize = 4 * 1024 * 1024;

/// Most codings accepted in one `Content-Encoding` header (`gzip, gzip, ...`).
pub const MAX_CODINGS: usize = 4;

/// Why decoding failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodingError {
    /// Not a gzip stream (bad magic, method or reserved flag bits).
    BadHeader,
    /// Truncated header, stream or trailer.
    Truncated,
    /// The DEFLATE data is malformed.
    Corrupt,
    /// CRC-32 or length in the gzip trailer does not match the data.
    BadChecksum,
    /// The output would exceed the limit.
    TooLarge,
}

impl From<InflateError> for EncodingError {
    fn from(e: InflateError) -> Self {
        match e {
            InflateError::OutputLimit => EncodingError::TooLarge,
            _ => EncodingError::Corrupt,
        }
    }
}

/// How much of a compressed body was recovered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completeness {
    /// The stream ended cleanly and its checksum matched.
    Complete,
    /// The input ended before the stream did (a download cut at the size cap, a
    /// dropped connection). Everything decoded up to there is in `data`.
    Cut,
    /// The output limit was reached: `data` is the first `max_output` bytes.
    TooLarge,
    /// The data went bad after the part in `data` (corrupt DEFLATE).
    Damaged,
    /// All data was decoded but the trailer's checksum or length is wrong.
    BadChecksum,
}

impl Completeness {
    fn of(e: InflateError) -> Completeness {
        match e {
            InflateError::Truncated => Completeness::Cut,
            InflateError::OutputLimit | InflateError::OutOfMemory => Completeness::TooLarge,
            InflateError::ChecksumMismatch => Completeness::BadChecksum,
            _ => Completeness::Damaged,
        }
    }

    /// The strict-API error for an incomplete result.
    fn into_error(self) -> Result<(), EncodingError> {
        match self {
            Completeness::Complete => Ok(()),
            Completeness::Cut => Err(EncodingError::Truncated),
            Completeness::TooLarge => Err(EncodingError::TooLarge),
            Completeness::Damaged => Err(EncodingError::Corrupt),
            Completeness::BadChecksum => Err(EncodingError::BadChecksum),
        }
    }
}

/// A decoded body, possibly only a prefix of the real one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub data: Vec<u8>,
    pub status: Completeness,
}

impl Decoded {
    fn into_strict(self) -> Result<Vec<u8>, EncodingError> {
        self.status.into_error().map(|()| self.data)
    }
}

const FHCRC: u8 = 0x02;
const FEXTRA: u8 = 0x04;
const FNAME: u8 = 0x08;
const FCOMMENT: u8 = 0x10;
const RESERVED: u8 = 0xE0;

/// Length of the gzip member header at the start of `data`.
fn gzip_header_len(data: &[u8]) -> Result<usize, EncodingError> {
    if data.len() < 10 {
        return Err(if data.len() >= 2 && data[..2] != [0x1F, 0x8B] {
            EncodingError::BadHeader
        } else {
            EncodingError::Truncated
        });
    }
    if data[0] != 0x1F || data[1] != 0x8B || data[2] != 8 {
        return Err(EncodingError::BadHeader);
    }
    let flags = data[3];
    if flags & RESERVED != 0 {
        return Err(EncodingError::BadHeader);
    }
    let mut pos = 10usize;
    if flags & FEXTRA != 0 {
        let len = usize::from(u16::from_le_bytes([
            *data.get(pos).ok_or(EncodingError::Truncated)?,
            *data.get(pos + 1).ok_or(EncodingError::Truncated)?,
        ]));
        pos = pos
            .checked_add(2 + len)
            .filter(|&p| p <= data.len())
            .ok_or(EncodingError::Truncated)?;
    }
    for flag in [FNAME, FCOMMENT] {
        if flags & flag != 0 {
            let nul = data
                .get(pos..)
                .and_then(|r| r.iter().position(|&b| b == 0))
                .ok_or(EncodingError::Truncated)?;
            pos += nul + 1;
        }
    }
    if flags & FHCRC != 0 {
        pos = pos.checked_add(2).ok_or(EncodingError::Truncated)?;
    }
    if pos > data.len() {
        return Err(EncodingError::Truncated);
    }
    Ok(pos)
}

/// Compress `data` as one gzip member (RFC 1952): DEFLATE with the fixed Huffman code, no name,
/// no timestamp (so the output only depends on the data).
pub fn gzip(data: &[u8]) -> Vec<u8> {
    let body = crate::format::deflate::deflate_fixed(data);
    let mut out = Vec::with_capacity(body.len() + 18);
    // magic, method 8, no flags, mtime 0, no extra flags, OS 255 (unknown)
    out.extend_from_slice(&[0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 255]);
    out.extend_from_slice(&body);
    out.extend_from_slice(&inflate::crc32(data).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

/// Decompress one gzip member (trailing garbage after the trailer is ignored,
/// as browsers do; further members are not followed). Strict: any problem,
/// including a cut stream, is an error; see [`gunzip_partial`] for the lenient
/// variant the browser uses.
pub fn gunzip(data: &[u8], max_output: usize) -> Result<Vec<u8>, EncodingError> {
    if data.len() < 18 {
        return Err(if data.len() >= 2 && data[..2] != [0x1F, 0x8B] {
            EncodingError::BadHeader
        } else {
            EncodingError::Truncated
        });
    }
    gunzip_partial(data, max_output)?.into_strict()
}

/// Decompress one gzip member, keeping whatever was decoded before a problem.
///
/// Only an unusable *start* is an `Err` (bad or incomplete header, or a stream
/// that failed before producing a single byte, reported as that failure). Once
/// bytes were decoded the result is `Ok` with a [`Completeness`] saying whether
/// they are the whole body: a download cut mid-stream, a bad trailer and a
/// corrupt tail all give the decoded prefix instead of nothing. The CRC is
/// checked only when the stream ended cleanly.
pub fn gunzip_partial(data: &[u8], max_output: usize) -> Result<Decoded, EncodingError> {
    let pos = gzip_header_len(data)?;
    let body = data.get(pos..).ok_or(EncodingError::Truncated)?;
    let (out, err, used) = inflate::inflate_consumed_partial(body, max_output);
    if let Some(e) = err {
        return finish_partial(out, e);
    }
    let Some(trailer) = body.get(used..used + 8) else {
        // The DEFLATE stream is complete but the 8 trailer bytes were cut off.
        return Ok(Decoded {
            data: out,
            status: Completeness::Cut,
        });
    };
    let crc = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let isize = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);
    let status = if crc != inflate::crc32(&out) || isize != out.len() as u32 {
        Completeness::BadChecksum
    } else {
        Completeness::Complete
    };
    Ok(Decoded { data: out, status })
}

/// A failed inflate: the prefix is the result unless there is none.
fn finish_partial(out: Vec<u8>, e: InflateError) -> Result<Decoded, EncodingError> {
    let status = Completeness::of(e);
    if out.is_empty() && status != Completeness::BadChecksum {
        // Nothing to show: report the failure itself.
        return Err(match status {
            Completeness::Cut => EncodingError::Truncated,
            Completeness::TooLarge => EncodingError::TooLarge,
            _ => EncodingError::Corrupt,
        });
    }
    Ok(Decoded { data: out, status })
}

/// Does `data` start with a plausible zlib header (RFC 1950)?
fn looks_like_zlib(data: &[u8]) -> bool {
    match data {
        [cmf, flg, ..] => {
            cmf & 0x0F == 8
                && cmf >> 4 <= 7
                && ((u32::from(*cmf) << 8) | u32::from(*flg)).is_multiple_of(31)
                && flg & 0x20 == 0
        }
        _ => false,
    }
}

/// `Content-Encoding: deflate` leniently: zlib-wrapped per RFC 9110, else bare
/// DEFLATE, keeping the decoded prefix of a cut or damaged stream. A zlib header
/// that is valid commits to zlib (so a cut zlib stream is not retried as raw and
/// turned into garbage) unless zlib decoded nothing, in which case the bytes get
/// one more chance as raw DEFLATE.
pub fn inflate_http_deflate_partial(
    data: &[u8],
    max_output: usize,
) -> Result<Decoded, EncodingError> {
    if looks_like_zlib(data)
        && let Ok((out, err)) = inflate::zlib_partial(data, max_output)
        && (!out.is_empty() || err.is_none())
    {
        let status = err.map_or(Completeness::Complete, Completeness::of);
        return Ok(Decoded { data: out, status });
    }
    let (out, err) = inflate::inflate_partial(data, max_output);
    match err {
        None => Ok(Decoded {
            data: out,
            status: Completeness::Complete,
        }),
        Some(e) => finish_partial(out, e),
    }
}

/// `Content-Encoding: deflate`, strict: zlib-wrapped per RFC 9110, else bare
/// DEFLATE.
pub fn inflate_http_deflate(data: &[u8], max_output: usize) -> Result<Vec<u8>, EncodingError> {
    match inflate::zlib_decompress(data, max_output) {
        Ok(v) => Ok(v),
        Err(InflateError::OutputLimit) => Err(EncodingError::TooLarge),
        Err(_) => Ok(inflate::inflate(data, max_output)?),
    }
}

/// How the response body is encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Identity,
    Gzip,
    Deflate,
    /// Anything else (`br`, `zstd`, ...): not decodable here.
    Unsupported,
}

impl Encoding {
    /// Parse a `Content-Encoding` header value holding ONE coding
    /// (case-insensitive); a list is [`Encoding::Unsupported`] here, see
    /// [`Encoding::parse_chain`].
    pub fn parse(value: &[u8]) -> Encoding {
        let v = value.trim_ascii();
        if v.is_empty() || v.eq_ignore_ascii_case(b"identity") {
            Encoding::Identity
        } else if v.eq_ignore_ascii_case(b"gzip") || v.eq_ignore_ascii_case(b"x-gzip") {
            Encoding::Gzip
        } else if v.eq_ignore_ascii_case(b"deflate") {
            Encoding::Deflate
        } else {
            Encoding::Unsupported
        }
    }

    /// Parse a whole `Content-Encoding` value: codings in the order they were
    /// applied by the server (`gzip, gzip` etc.), `identity` and empty items
    /// dropped. More than [`MAX_CODINGS`] items, or any unknown one (`br`,
    /// `zstd`, ...), makes the chain `[Unsupported]`: it cannot be decoded
    /// honestly.
    pub fn parse_chain(value: &[u8]) -> Vec<Encoding> {
        let mut chain = Vec::new();
        for item in value.split(|&b| b == b',') {
            match Encoding::parse(item) {
                Encoding::Identity => {}
                Encoding::Unsupported => return alloc::vec![Encoding::Unsupported],
                e => chain.push(e),
            }
        }
        if chain.len() > MAX_CODINGS {
            return alloc::vec![Encoding::Unsupported];
        }
        chain
    }
}

/// Decode `body` according to `enc`, within [`MAX_DECODED_BYTES`]. Strict.
pub fn decode_body(enc: Encoding, body: &[u8]) -> Result<Vec<u8>, EncodingError> {
    decode_chain_partial(&[enc], body)?.into_strict()
}

/// Decode `body` through a coding chain (`Content-Encoding` order: the last
/// item is undone first), within [`MAX_DECODED_BYTES`] per stage, keeping the
/// decoded prefix of a cut or damaged stream. The result's status is that of the
/// first stage that was not [`Completeness::Complete`].
pub fn decode_chain_partial(chain: &[Encoding], body: &[u8]) -> Result<Decoded, EncodingError> {
    let mut cur = Decoded {
        data: body.to_vec(),
        status: Completeness::Complete,
    };
    for enc in chain.iter().rev() {
        let next = match enc {
            Encoding::Identity => continue,
            Encoding::Gzip => gunzip_partial(&cur.data, MAX_DECODED_BYTES)?,
            Encoding::Deflate => inflate_http_deflate_partial(&cur.data, MAX_DECODED_BYTES)?,
            Encoding::Unsupported => return Err(EncodingError::BadHeader),
        };
        let status = if cur.status == Completeness::Complete {
            next.status
        } else {
            cur.status
        };
        cur = Decoded {
            data: next.data,
            status,
        };
    }
    Ok(cur)
}

#[cfg(test)]
mod tests;
