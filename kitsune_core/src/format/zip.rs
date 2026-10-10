//! ZIP archives: reading (list and extract `stored` and `deflate` entries) and writing.
//!
//! Safe and bounded like the rest of `format`: no function panics on any input; the reader
//! trusts nothing in the file (every offset and length is checked against the data, the number
//! of entries and the size of one entry are capped, and what an entry expands to can never
//! exceed the size its directory record declares, so a "zip bomb" costs at most that size).
//!
//! Not supported, reported as [`ZipError::Unsupported`] and never guessed at: encryption, ZIP64
//! (archives or entries of 4 GiB and more), multi-disk archives and any method besides stored
//! and deflate.
//!
//! Names are made safe for extraction by [`safe_path`]: `\` is a separator, `..` and absolute
//! paths are refused, so an archive cannot write outside the folder it is extracted into.

use crate::format::deflate::deflate_fixed;
use crate::format::inflate::{crc32, inflate};
use crate::format::unixtime::DateTime;
use alloc::string::String;
use alloc::vec::Vec;

/// Most entries read from one archive.
pub const MAX_ENTRIES: usize = 4096;
/// The longest name kept.
pub const MAX_NAME: usize = 255;

const EOCD_SIG: u32 = 0x0605_4B50;
const CEN_SIG: u32 = 0x0201_4B50;
const LOC_SIG: u32 = 0x0403_4B50;
const EOCD_LEN: usize = 22;
const CEN_LEN: usize = 46;
const LOC_LEN: usize = 30;
const FLAG_ENCRYPTED: u16 = 1;
const FLAG_UTF8: u16 = 1 << 11;

/// Why an archive or an entry was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZipError {
    /// No end-of-directory record: not a ZIP file (or cut short).
    NotZip,
    /// A record or data runs past the end of the file.
    Truncated,
    /// Encrypted, ZIP64, multi-disk or an unknown compression method.
    Unsupported,
    /// The directory or the data is inconsistent.
    Corrupt,
    /// The checksum or the size of the extracted data does not match the directory.
    BadChecksum,
    /// The entry expands to more than the limit given.
    TooLarge,
    /// More than [`MAX_ENTRIES`] entries.
    TooMany,
}

/// One entry of the central directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The name as stored (`/` separators, a trailing `/` for a folder).
    pub name: String,
    pub method: u16,
    pub crc: u32,
    pub compressed: u32,
    pub size: u32,
    pub offset: u32,
    /// Last-modified time in the MS-DOS format (time, date).
    pub dos: (u16, u16),
    pub is_dir: bool,
    flags: u16,
}

fn u16_at(d: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*d.get(i)?, *d.get(i + 1)?]))
}

fn u32_at(d: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *d.get(i)?,
        *d.get(i + 1)?,
        *d.get(i + 2)?,
        *d.get(i + 3)?,
    ]))
}

/// Names written without the UTF-8 flag are CP437 by the book, but nearly every program wrote
/// the local code page; keep valid UTF-8 and read anything else as Latin-1 so nothing is lost.
fn decode_name(raw: &[u8]) -> String {
    match core::str::from_utf8(raw) {
        Ok(s) => String::from(s),
        Err(_) => raw.iter().map(|&b| char::from(b)).collect(),
    }
}

/// Read the central directory.
pub fn read_dir(data: &[u8]) -> Result<Vec<Entry>, ZipError> {
    if data.len() < EOCD_LEN {
        return Err(ZipError::NotZip);
    }
    // The end record is the last 22 bytes unless a comment (at most 65535 bytes) follows it.
    let lowest = data.len().saturating_sub(EOCD_LEN + 65_535);
    let eocd = (lowest..=data.len() - EOCD_LEN)
        .rev()
        .find(|&i| u32_at(data, i) == Some(EOCD_SIG))
        .ok_or(ZipError::NotZip)?;
    let disk = u16_at(data, eocd + 4).ok_or(ZipError::Truncated)?;
    let cd_disk = u16_at(data, eocd + 6).ok_or(ZipError::Truncated)?;
    let here = u16_at(data, eocd + 8).ok_or(ZipError::Truncated)?;
    let total = u16_at(data, eocd + 10).ok_or(ZipError::Truncated)?;
    let cd_size = u32_at(data, eocd + 12).ok_or(ZipError::Truncated)?;
    let cd_off = u32_at(data, eocd + 16).ok_or(ZipError::Truncated)?;
    if disk != 0 || cd_disk != 0 || here != total {
        return Err(ZipError::Unsupported);
    }
    if total == 0xFFFF || cd_size == u32::MAX || cd_off == u32::MAX {
        return Err(ZipError::Unsupported); // ZIP64
    }
    if usize::from(total) > MAX_ENTRIES {
        return Err(ZipError::TooMany);
    }
    let start = cd_off as usize;
    let end = start
        .checked_add(cd_size as usize)
        .filter(|&e| e <= eocd)
        .ok_or(ZipError::Corrupt)?;
    let cd = &data[start..end];
    let mut out = Vec::with_capacity(usize::from(total));
    let mut p = 0;
    for _ in 0..total {
        if u32_at(cd, p) != Some(CEN_SIG) {
            return Err(ZipError::Corrupt);
        }
        let flags = u16_at(cd, p + 8).ok_or(ZipError::Truncated)?;
        let method = u16_at(cd, p + 10).ok_or(ZipError::Truncated)?;
        let time = u16_at(cd, p + 12).ok_or(ZipError::Truncated)?;
        let date = u16_at(cd, p + 14).ok_or(ZipError::Truncated)?;
        let crc = u32_at(cd, p + 16).ok_or(ZipError::Truncated)?;
        let compressed = u32_at(cd, p + 20).ok_or(ZipError::Truncated)?;
        let size = u32_at(cd, p + 24).ok_or(ZipError::Truncated)?;
        let name_len = usize::from(u16_at(cd, p + 28).ok_or(ZipError::Truncated)?);
        let extra_len = usize::from(u16_at(cd, p + 30).ok_or(ZipError::Truncated)?);
        let comment_len = usize::from(u16_at(cd, p + 32).ok_or(ZipError::Truncated)?);
        let offset = u32_at(cd, p + 42).ok_or(ZipError::Truncated)?;
        let name = cd
            .get(p + CEN_LEN..p + CEN_LEN + name_len)
            .ok_or(ZipError::Truncated)?;
        p = (p + CEN_LEN)
            .checked_add(name_len + extra_len + comment_len)
            .filter(|&n| n <= cd.len())
            .ok_or(ZipError::Truncated)?;
        let mut name = decode_name(name);
        if name.len() > MAX_NAME {
            return Err(ZipError::Corrupt);
        }
        let is_dir = name.ends_with('/') || name.ends_with('\\');
        if is_dir && size != 0 {
            return Err(ZipError::Corrupt);
        }
        if name.contains('\\') {
            name = name.replace('\\', "/");
        }
        out.push(Entry {
            name,
            method,
            crc,
            compressed,
            size,
            offset,
            dos: (time, date),
            is_dir,
            flags,
        });
    }
    Ok(out)
}

/// The bytes of `e`, checked against its size and CRC-32. `max` caps the size an entry may
/// have (the directory's word is not trusted before it is checked against `max`).
pub fn extract(data: &[u8], e: &Entry, max: usize) -> Result<Vec<u8>, ZipError> {
    if e.flags & FLAG_ENCRYPTED != 0 || (e.method != 0 && e.method != 8) {
        return Err(ZipError::Unsupported);
    }
    if e.size == u32::MAX || e.compressed == u32::MAX || e.offset == u32::MAX {
        return Err(ZipError::Unsupported);
    }
    if e.is_dir {
        return Ok(Vec::new());
    }
    let size = e.size as usize;
    if size > max {
        return Err(ZipError::TooLarge);
    }
    let at = e.offset as usize;
    if u32_at(data, at) != Some(LOC_SIG) {
        return Err(ZipError::Corrupt);
    }
    let name_len = usize::from(u16_at(data, at + 26).ok_or(ZipError::Truncated)?);
    let extra_len = usize::from(u16_at(data, at + 28).ok_or(ZipError::Truncated)?);
    let body = at + LOC_LEN + name_len + extra_len;
    let packed = data
        .get(
            body..body
                .checked_add(e.compressed as usize)
                .ok_or(ZipError::Corrupt)?,
        )
        .ok_or(ZipError::Truncated)?;
    let out = match e.method {
        0 => {
            if e.compressed != e.size {
                return Err(ZipError::Corrupt);
            }
            packed.to_vec()
        }
        _ => inflate(packed, size).map_err(|err| match err {
            crate::format::inflate::InflateError::OutputLimit => ZipError::BadChecksum,
            crate::format::inflate::InflateError::Truncated => ZipError::Truncated,
            _ => ZipError::Corrupt,
        })?,
    };
    if out.len() != size || crc32(&out) != e.crc {
        return Err(ZipError::BadChecksum);
    }
    Ok(out)
}

/// A name made safe to create under a folder: `\` becomes `/`, empty and `.` parts and a leading
/// `/` are dropped, and `None` is returned for `..`, a drive letter (`C:`), a control character
/// or nothing at all. A trailing `/` (a folder) is kept.
pub fn safe_path(name: &str) -> Option<String> {
    let name = name.replace('\\', "/");
    let mut out = String::new();
    for part in name.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.contains(':') || part.chars().any(|c| c.is_control()) {
            return None;
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(part);
    }
    if out.is_empty() {
        return None;
    }
    if name.ends_with('/') {
        out.push('/');
    }
    Some(out)
}

// ---- writing ----

/// The MS-DOS time and date of `t` (two-second steps, years 1980..=2107).
pub fn dos_time(t: &DateTime) -> (u16, u16) {
    let year = t.year.clamp(1980, 2107) - 1980;
    let time = (u16::from(t.hour) << 11) | (u16::from(t.minute) << 5) | u16::from(t.second / 2);
    let date = ((year as u16) << 9) | (u16::from(t.month) << 5) | u16::from(t.day);
    (time, date)
}

/// The civil time of an MS-DOS time and date (`None` for a nonsense date).
pub fn from_dos(time: u16, date: u16) -> Option<DateTime> {
    let t = DateTime {
        year: i32::from(date >> 9) + 1980,
        month: ((date >> 5) & 15) as u8,
        day: (date & 31) as u8,
        hour: (time >> 11) as u8,
        minute: ((time >> 5) & 63) as u8,
        second: ((time & 31) * 2) as u8,
    };
    let ok = (1..=12).contains(&t.month)
        && t.day >= 1
        && t.day <= crate::format::unixtime::days_in_month(t.year, t.month)
        && t.hour < 24
        && t.minute < 60
        && t.second < 60;
    ok.then_some(t)
}

struct Written {
    name: String,
    method: u16,
    crc: u32,
    compressed: u32,
    size: u32,
    offset: u32,
    dos: (u16, u16),
    flags: u16,
}

/// Builds an archive in memory.
#[derive(Default)]
pub struct ZipWriter {
    out: Vec<u8>,
    entries: Vec<Written>,
}

impl ZipWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Entries added so far.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Bytes written so far (the directory comes with [`ZipWriter::finish`]).
    pub fn bytes(&self) -> usize {
        self.out.len()
    }

    fn add(&mut self, name: &str, data: &[u8], dos: (u16, u16)) -> Result<(), ZipError> {
        if self.entries.len() >= MAX_ENTRIES || name.len() > MAX_NAME {
            return Err(ZipError::TooMany);
        }
        if data.len() >= u32::MAX as usize || self.out.len() >= u32::MAX as usize - data.len() {
            return Err(ZipError::TooLarge);
        }
        let (method, packed): (u16, Vec<u8>) = if data.is_empty() || name.ends_with('/') {
            (0, Vec::new())
        } else {
            let p = deflate_fixed(data);
            // The encoder already falls back to stored blocks; keep the entry stored when
            // deflating did not pay.
            if p.len() < data.len() {
                (8, p)
            } else {
                (0, data.to_vec())
            }
        };
        let flags = if name.is_ascii() { 0 } else { FLAG_UTF8 };
        let offset = self.out.len() as u32;
        let crc = crc32(data);
        let o = &mut self.out;
        o.extend_from_slice(&LOC_SIG.to_le_bytes());
        o.extend_from_slice(&20u16.to_le_bytes()); // version needed
        o.extend_from_slice(&flags.to_le_bytes());
        o.extend_from_slice(&method.to_le_bytes());
        o.extend_from_slice(&dos.0.to_le_bytes());
        o.extend_from_slice(&dos.1.to_le_bytes());
        o.extend_from_slice(&crc.to_le_bytes());
        o.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        o.extend_from_slice(&(data.len() as u32).to_le_bytes());
        o.extend_from_slice(&(name.len() as u16).to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes()); // extra
        o.extend_from_slice(name.as_bytes());
        o.extend_from_slice(&packed);
        self.entries.push(Written {
            name: String::from(name),
            method,
            crc,
            compressed: packed.len() as u32,
            size: data.len() as u32,
            offset,
            dos,
            flags,
        });
        Ok(())
    }

    /// Add a file. `name` must already be a safe relative path (see [`safe_path`]).
    pub fn add_file(&mut self, name: &str, data: &[u8], dos: (u16, u16)) -> Result<(), ZipError> {
        self.add(name, data, dos)
    }

    /// Add a folder (`name` gets its trailing `/`).
    pub fn add_dir(&mut self, name: &str, dos: (u16, u16)) -> Result<(), ZipError> {
        let mut n = String::from(name);
        if !n.ends_with('/') {
            n.push('/');
        }
        self.add(&n, &[], dos)
    }

    /// Append the central directory and return the archive.
    pub fn finish(mut self) -> Vec<u8> {
        let cd_start = self.out.len();
        let mut cd = Vec::new();
        for e in &self.entries {
            cd.extend_from_slice(&CEN_SIG.to_le_bytes());
            cd.extend_from_slice(&0x031Eu16.to_le_bytes()); // made by: Unix, 3.0
            cd.extend_from_slice(&20u16.to_le_bytes());
            cd.extend_from_slice(&e.flags.to_le_bytes());
            cd.extend_from_slice(&e.method.to_le_bytes());
            cd.extend_from_slice(&e.dos.0.to_le_bytes());
            cd.extend_from_slice(&e.dos.1.to_le_bytes());
            cd.extend_from_slice(&e.crc.to_le_bytes());
            cd.extend_from_slice(&e.compressed.to_le_bytes());
            cd.extend_from_slice(&e.size.to_le_bytes());
            cd.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            cd.extend_from_slice(&[0; 2 + 2 + 2 + 2]); // extra, comment, disk, internal attrs
            let attrs: u32 = if e.name.ends_with('/') {
                (0o040_755 << 16) | 0x10
            } else {
                0o100_644 << 16
            };
            cd.extend_from_slice(&attrs.to_le_bytes());
            cd.extend_from_slice(&e.offset.to_le_bytes());
            cd.extend_from_slice(e.name.as_bytes());
        }
        let n = self.entries.len() as u16;
        self.out.extend_from_slice(&cd);
        let o = &mut self.out;
        o.extend_from_slice(&EOCD_SIG.to_le_bytes());
        o.extend_from_slice(&[0; 4]); // this disk, directory disk
        o.extend_from_slice(&n.to_le_bytes());
        o.extend_from_slice(&n.to_le_bytes());
        o.extend_from_slice(&(cd.len() as u32).to_le_bytes());
        o.extend_from_slice(&(cd_start as u32).to_le_bytes());
        o.extend_from_slice(&0u16.to_le_bytes()); // comment length
        self.out
    }
}

#[cfg(test)]
mod tests;
