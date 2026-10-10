//! ZIP archives in the file manager: "extract here" and "compress", over a [`Backend`].
//!
//! Extraction goes into a new folder named after the archive (never over what is there), skips
//! entries whose names would leave that folder ([`zip::safe_path`]) and refuses an archive that
//! would expand past [`MAX_TOTAL`]. Compression keeps the structure under the folder holding the
//! items.

use super::{Backend, EntryKind, VfsError, vfs};
use crate::format::unixtime::DateTime;
use crate::format::zip::{self, ZipError, ZipWriter};
use alloc::string::String;
use alloc::vec::Vec;

/// Largest archive read or written, in bytes.
pub const MAX_ARCHIVE: usize = 32 << 20;
/// Most an archive may expand to, in total.
pub const MAX_TOTAL: usize = 64 << 20;
/// Largest single file extracted.
pub const MAX_FILE: usize = 16 << 20;
const MAX_DEPTH: usize = 32;

/// Why an archive operation failed as a whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveError {
    Fs(VfsError),
    Zip(ZipError),
}

impl From<VfsError> for ArchiveError {
    fn from(e: VfsError) -> Self {
        ArchiveError::Fs(e)
    }
}

impl From<ZipError> for ArchiveError {
    fn from(e: ZipError) -> Self {
        ArchiveError::Zip(e)
    }
}

/// What an extraction did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extracted {
    /// The folder created (an absolute path).
    pub folder: Vec<u8>,
    /// Files written.
    pub files: usize,
    /// Entries left out: unsafe names, unsupported entries, damaged data or a full disk.
    pub skipped: usize,
}

/// Is `name` a ZIP archive by its extension?
pub fn is_zip(name: &[u8]) -> bool {
    name.len() > 4 && name[name.len() - 4..].eq_ignore_ascii_case(b".zip")
}

fn make_dirs<B: Backend + ?Sized>(be: &mut B, dir: &[u8], now: u64) -> Result<(), VfsError> {
    let mut cur: Vec<u8> = Vec::new();
    for comp in dir.split(|&b| b == b'/').filter(|c| !c.is_empty()) {
        cur.push(b'/');
        cur.extend_from_slice(comp);
        match be.stat(&cur) {
            Ok(i) if i.kind == EntryKind::Dir => {}
            Ok(_) => return Err(VfsError::NotDir),
            Err(_) => be.mkdir(&cur, now)?,
        }
    }
    Ok(())
}

/// Extract the archive at `zip_path` into a new folder beside it.
pub fn extract_zip<B: Backend + ?Sized>(
    be: &mut B,
    zip_path: &[u8],
    now: u64,
) -> Result<Extracted, ArchiveError> {
    let info = be.stat(zip_path)?;
    if info.size > MAX_ARCHIVE as u64 {
        return Err(ZipError::TooLarge.into());
    }
    let data = be.read_file(zip_path)?;
    let entries = zip::read_dir(&data)?;
    let declared: u64 = entries.iter().map(|e| u64::from(e.size)).sum();
    if declared > MAX_TOTAL as u64 {
        return Err(ZipError::TooLarge.into());
    }
    let dir = vfs::parent(zip_path);
    let base = vfs::base_name(zip_path);
    let stem = &base[..base.len() - if is_zip(base) { 4 } else { 0 }];
    let name = vfs::unique_name(stem, |n| vfs::exists(be, &vfs::join(&dir, n)));
    let folder = vfs::join(&dir, &name);
    be.mkdir(&folder, now)?;
    let mut done = Extracted {
        folder: folder.clone(),
        files: 0,
        skipped: 0,
    };
    for e in &entries {
        let Some(rel) = zip::safe_path(&e.name) else {
            done.skipped += 1;
            continue;
        };
        let target = vfs::join(&folder, rel.trim_end_matches('/').as_bytes());
        let result = if e.is_dir {
            make_dirs(be, &target, now).map_err(ArchiveError::from)
        } else {
            zip::extract(&data, e, MAX_FILE)
                .map_err(ArchiveError::from)
                .and_then(|bytes| {
                    make_dirs(be, &vfs::parent(&target), now)?;
                    be.write_file(&target, &bytes, now)?;
                    Ok(())
                })
        };
        match result {
            Ok(()) if !e.is_dir => done.files += 1,
            Ok(()) => {}
            // A full disk stops everything: the rest would fail the same way.
            Err(ArchiveError::Fs(VfsError::NoSpace | VfsError::NoInodes)) => {
                done.skipped += entries.len() - done.files;
                break;
            }
            Err(_) => done.skipped += 1,
        }
    }
    Ok(done)
}

fn add_tree<B: Backend + ?Sized>(
    be: &mut B,
    w: &mut ZipWriter,
    path: &[u8],
    name: &str,
    dos: (u16, u16),
    depth: usize,
) -> Result<(), ArchiveError> {
    let info = be.stat(path)?;
    if info.kind == EntryKind::File {
        if info.size > MAX_ARCHIVE as u64 || w.bytes() as u64 + info.size > MAX_ARCHIVE as u64 {
            return Err(ZipError::TooLarge.into());
        }
        let data = be.read_file(path)?;
        w.add_file(name, &data, dos)?;
        return Ok(());
    }
    w.add_dir(name, dos)?;
    if depth >= MAX_DEPTH {
        return Ok(());
    }
    let mut kids = be.names(path)?;
    kids.sort_by(|a, b| a.0.cmp(&b.0));
    for (kid, _) in kids {
        // A name that is not UTF-8 cannot go in an archive (the flag says names are UTF-8).
        let Ok(kid_name) = core::str::from_utf8(&kid) else {
            continue;
        };
        add_tree(
            be,
            w,
            &vfs::join(path, &kid),
            &alloc::format!("{name}/{kid_name}"),
            dos,
            depth + 1,
        )?;
    }
    Ok(())
}

/// Compress `items` (absolute paths in one folder) into a new `.zip` in that folder and return
/// its path. `fallback` names the archive when there is more than one item.
pub fn compress<B: Backend + ?Sized>(
    be: &mut B,
    items: &[Vec<u8>],
    fallback: &str,
    now: u64,
) -> Result<Vec<u8>, ArchiveError> {
    let first = items.first().ok_or(VfsError::NotFound)?;
    let dir = vfs::parent(first);
    let dt = DateTime::from_unix(now);
    let dos = zip::dos_time(&dt);
    let mut w = ZipWriter::new();
    for p in items {
        let name = vfs::base_name(p);
        let Ok(name) = core::str::from_utf8(name) else {
            return Err(VfsError::InvalidName.into());
        };
        add_tree(be, &mut w, p, name, dos, 0)?;
    }
    let stem: String = if items.len() == 1 {
        String::from_utf8_lossy(vfs::base_name(first)).into_owned()
    } else {
        String::from(fallback)
    };
    let mut wanted = stem.into_bytes();
    wanted.extend_from_slice(b".zip");
    let name = vfs::unique_name(&wanted, |n| vfs::exists(be, &vfs::join(&dir, n)));
    let out = vfs::join(&dir, &name);
    be.write_file(&out, &w.finish(), now)?;
    Ok(out)
}
