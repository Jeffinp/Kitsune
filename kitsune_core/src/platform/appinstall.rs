//! Installing, removing and listing apps in `/apps/<id>.wasm`.
//!
//! `install` validates **before** it writes: the module's section table, the
//! manifest (grammar, permissions, quotas under the system ceilings), the icon,
//! and that the id is not already installed. The file is written under a
//! temporary name and renamed into place, so an interrupted install never leaves
//! a half-written `<id>.wasm` for the launcher to trip over.

use crate::format::image::{Filter, Image};
use crate::i18n::{self, Arg, Lang};
use crate::platform::appfs::{AppFs, FsError, Kind};
use crate::platform::appmanifest::{self, Manifest, PackageError, valid_id};
use crate::tk;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// Where installed apps live.
pub const APPS_DIR: &str = "/apps";
const TMP: &str = "/apps/.install.tmp";
/// Largest package the installer accepts.
pub const MAX_PACKAGE_BYTES: usize = 4 << 20;
/// Most apps in the catalog (a bound for the launcher UI and the heap).
pub const MAX_APPS: usize = 64;
/// Launcher icon edge, in pixels.
pub const ICON_SIZE: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallError {
    TooLarge,
    Package(PackageError),
    /// An app with this id is already installed.
    Duplicate,
    NotInstalled,
    BadId,
    /// Too many apps installed.
    Full,
    Fs(FsError),
}

impl From<FsError> for InstallError {
    fn from(e: FsError) -> Self {
        InstallError::Fs(e)
    }
}

impl InstallError {
    /// The reason in `lang`, in words for the person (the [`Display`](fmt::Display) text is for
    /// the log).
    pub fn message_in(&self, lang: Lang) -> String {
        let key = match self {
            InstallError::Package(e) => return e.message_in(lang),
            InstallError::TooLarge => tk!("apps.err.too_large"),
            InstallError::Duplicate => tk!("apps.err.duplicate"),
            InstallError::NotInstalled => tk!("apps.err.not_installed"),
            InstallError::BadId => tk!("apps.err.bad_id"),
            InstallError::Full => tk!("apps.err.full"),
            InstallError::Fs(e) => {
                return i18n::tr_fmt_in(
                    lang,
                    tk!("apps.err.fs"),
                    &[("why", Arg::Str(i18n::tr_in(lang, e.key())))],
                );
            }
        };
        String::from(i18n::tr_in(lang, key))
    }

    /// The reason in the language in effect.
    pub fn message(&self) -> String {
        self.message_in(i18n::lang())
    }
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstallError::TooLarge => f.write_str("package is larger than 4 MiB"),
            InstallError::Package(e) => e.fmt(f),
            InstallError::Duplicate => f.write_str("an app with this id is already installed"),
            InstallError::NotInstalled => f.write_str("app is not installed"),
            InstallError::BadId => f.write_str("invalid app id"),
            InstallError::Full => f.write_str("too many installed apps"),
            InstallError::Fs(e) => write!(f, "file system: {e}"),
        }
    }
}

fn package_path(id: &str) -> String {
    let mut p = String::from(APPS_DIR);
    p.push('/');
    p.push_str(id);
    p.push_str(".wasm");
    p
}

/// Is `<id>` installed?
pub fn is_installed(fs: &mut dyn AppFs, id: &str) -> bool {
    valid_id(id)
        && matches!(
            fs.stat(&package_path(id)),
            Ok(s) if s.kind == Kind::File
        )
}

/// Validates `wasm` without touching the filesystem.
pub fn check(wasm: &[u8]) -> Result<Manifest, InstallError> {
    if wasm.len() > MAX_PACKAGE_BYTES {
        return Err(InstallError::TooLarge);
    }
    appmanifest::parse_package(wasm)
        .map(|p| p.manifest)
        .map_err(InstallError::Package)
}

/// Installs `wasm` as `/apps/<id>.wasm`. Refuses an invalid package, a duplicate
/// id, and a full catalog; nothing is written in those cases.
pub fn install(fs: &mut dyn AppFs, wasm: &[u8]) -> Result<Manifest, InstallError> {
    let manifest = check(wasm)?;
    fs.mkdir_all(APPS_DIR)?;
    if is_installed(fs, &manifest.id) {
        return Err(InstallError::Duplicate);
    }
    if installed_ids(fs)?.len() >= MAX_APPS {
        return Err(InstallError::Full);
    }
    let _ = fs.remove(TMP); // leftover of an interrupted install
    fs.create(TMP)?;
    let result =
        write_all(fs, TMP, wasm).and_then(|()| fs.rename(TMP, &package_path(&manifest.id)));
    if let Err(e) = result {
        let _ = fs.remove(TMP);
        return Err(e.into());
    }
    Ok(manifest)
}

fn write_all(fs: &mut dyn AppFs, path: &str, data: &[u8]) -> Result<(), FsError> {
    let mut off = 0usize;
    while off < data.len() {
        let n = (data.len() - off).min(32 * 1024);
        let w = fs.write_at(path, off as u64, &data[off..off + n])?;
        if w == 0 {
            return Err(FsError::Io);
        }
        off += w;
    }
    Ok(())
}

/// Removes an installed app's package. Its data in `/data/<id>` is kept.
pub fn remove(fs: &mut dyn AppFs, id: &str) -> Result<(), InstallError> {
    if !valid_id(id) {
        return Err(InstallError::BadId);
    }
    match fs.remove(&package_path(id)) {
        Ok(()) => Ok(()),
        Err(FsError::NotFound) => Err(InstallError::NotInstalled),
        Err(e) => Err(e.into()),
    }
}

/// Ids of the installed packages (file names `<id>.wasm`, in name order).
pub fn installed_ids(fs: &mut dyn AppFs) -> Result<Vec<String>, FsError> {
    let mut ids = Vec::new();
    for i in 0..MAX_APPS + 8 {
        match fs.read_dir(APPS_DIR, i) {
            Ok(Some(e)) => {
                if e.kind == Kind::File
                    && let Some(id) = e.name.strip_suffix(".wasm")
                    && valid_id(id)
                {
                    ids.push(String::from(id));
                }
            }
            Ok(None) => break,
            Err(FsError::NotFound) => break,
            Err(e) => return Err(e),
        }
    }
    Ok(ids)
}

/// Reads an installed package back.
pub fn read_package(fs: &mut dyn AppFs, id: &str) -> Result<Vec<u8>, InstallError> {
    if !valid_id(id) {
        return Err(InstallError::BadId);
    }
    let path = package_path(id);
    let size = match fs.stat(&path) {
        Ok(s) if s.kind == Kind::File => s.size as usize,
        Ok(_) => return Err(InstallError::NotInstalled),
        Err(FsError::NotFound) => return Err(InstallError::NotInstalled),
        Err(e) => return Err(e.into()),
    };
    if size > MAX_PACKAGE_BYTES {
        return Err(InstallError::TooLarge);
    }
    let mut buf = Vec::new();
    buf.try_reserve_exact(size)
        .map_err(|_| InstallError::Fs(FsError::NoSpace))?;
    buf.resize(size, 0);
    let mut off = 0;
    while off < size {
        let n = fs.read_at(&path, off as u64, &mut buf[off..])?;
        if n == 0 {
            return Err(InstallError::Fs(FsError::Io));
        }
        off += n;
    }
    Ok(buf)
}

/// Reads at `off`, looping over short reads, until `buf` is full. `Ok(false)` at the end of the file.
fn read_full_at(fs: &mut dyn AppFs, path: &str, off: u64, buf: &mut [u8]) -> Result<bool, FsError> {
    let mut got = 0;
    while got < buf.len() {
        let n = fs.read_at(path, off + got as u64, &mut buf[got..])?;
        if n == 0 {
            return Ok(false);
        }
        got += n;
    }
    Ok(true)
}

/// A compact module made of the header and the manifest and icon sections of `/apps/<id>.wasm`,
/// read without touching the code: [`appmanifest::parse_package`] gives the same answer on it as on
/// the whole file. The launcher catalog only needs those two sections, and a package can be hundreds
/// of kilobytes of code (a 590 KB one meant ~1200 sectors of disk reads at every boot).
///
/// It walks the section table with the checks of [`crate::platform::wasmsec`] (header, section ids,
/// at most `MAX_SECTIONS` sections, sizes inside the file, custom names inside their payload), reading
/// only each section's first bytes, and copies only the sections named like the manifest or the icon.
pub fn read_package_meta(fs: &mut dyn AppFs, id: &str) -> Result<Vec<u8>, InstallError> {
    use crate::platform::wasmsec::{self, HEADER, MAX_SECTIONS};
    const NAMES: [&str; 4] = [
        appmanifest::MANIFEST_SECTION,
        appmanifest::ICON_SECTION,
        appmanifest::LEGACY_MANIFEST_SECTION,
        appmanifest::LEGACY_ICON_SECTION,
    ];
    // Longest name worth reading: the longest of NAMES, plus its LEB length byte.
    const PEEK: usize = 32;
    let bad = |e: wasmsec::WasmError| InstallError::Package(PackageError::Wasm(e));
    if !valid_id(id) {
        return Err(InstallError::BadId);
    }
    let path = package_path(id);
    let size = match fs.stat(&path) {
        Ok(s) if s.kind == Kind::File => s.size,
        Ok(_) => return Err(InstallError::NotInstalled),
        Err(FsError::NotFound) => return Err(InstallError::NotInstalled),
        Err(e) => return Err(e.into()),
    };
    if size > MAX_PACKAGE_BYTES as u64 {
        return Err(InstallError::TooLarge);
    }
    let mut head = [0u8; 8];
    if size < 8 || !read_full_at(fs, &path, 0, &mut head)? {
        return Err(bad(wasmsec::WasmError::TooShort));
    }
    if head[..4] != HEADER[..4] {
        return Err(bad(wasmsec::WasmError::BadMagic));
    }
    if head[4..8] != HEADER[4..8] {
        return Err(bad(wasmsec::WasmError::BadVersion));
    }
    let mut out = HEADER.to_vec();
    let mut pos = 8u64;
    let mut count = 0usize;
    while pos < size {
        if count >= MAX_SECTIONS {
            return Err(bad(wasmsec::WasmError::TooManySections));
        }
        count += 1;
        // The section id, its size (LEB128, at most 5 bytes) and the start of a custom name.
        let want = ((size - pos) as usize).min(6 + PEEK);
        let mut hdr = [0u8; 6 + PEEK];
        if !read_full_at(fs, &path, pos, &mut hdr[..want])? {
            return Err(InstallError::Fs(FsError::Io));
        }
        let kind = hdr[0];
        if kind > wasmsec::MAX_SECTION_ID {
            return Err(bad(wasmsec::WasmError::BadSectionId));
        }
        let (len, after) = wasmsec::read_leb_u32(&hdr[..want], 1).map_err(bad)?;
        let end = pos + after as u64 + u64::from(len);
        if end > size {
            return Err(bad(wasmsec::WasmError::Truncated));
        }
        if kind == 0 {
            let (nlen, name_at) = wasmsec::read_leb_u32(&hdr[..want], after).map_err(bad)?;
            if u64::from(nlen) + (name_at - after) as u64 > u64::from(len) {
                return Err(bad(wasmsec::WasmError::Truncated));
            }
            let name_end = name_at + nlen as usize;
            let keep = name_end <= want
                && core::str::from_utf8(&hdr[name_at..name_end]).is_ok_and(|n| NAMES.contains(&n));
            if keep {
                let total = (end - pos) as usize;
                let at = out.len();
                out.try_reserve_exact(total)
                    .map_err(|_| InstallError::Fs(FsError::NoSpace))?;
                out.resize(at + total, 0);
                if !read_full_at(fs, &path, pos, &mut out[at..])? {
                    return Err(InstallError::Fs(FsError::Io));
                }
            }
        }
        pos = end;
    }
    Ok(out)
}

/// A catalog entry for the launcher: the manifest and a 24x24 icon.
#[derive(Debug)]
pub struct CatalogEntry {
    pub manifest: Manifest,
    /// `ICON_SIZE * ICON_SIZE` pixels, `0xAARRGGBB`; `None` = use the default icon.
    pub icon: Option<Vec<u32>>,
}

/// Scales a package icon to the launcher size.
pub fn launcher_icon(img: &Image) -> Option<Vec<u32>> {
    let scaled = img.resize(ICON_SIZE, ICON_SIZE, Filter::Auto).ok()?;
    let mut px = Vec::with_capacity(ICON_SIZE * ICON_SIZE);
    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            px.push(scaled.get(x, y)?);
        }
    }
    Some(px)
}

/// Reads every installed package and builds the launcher catalog. Packages that
/// no longer validate, or whose manifest id differs from the file name, are
/// skipped (never trusted just because they sit in `/apps`).
pub fn load_catalog(fs: &mut dyn AppFs) -> Vec<CatalogEntry> {
    load_catalog_in(fs, i18n::lang())
}

/// [`load_catalog`] sorted by the names in `lang`.
pub fn load_catalog_in(fs: &mut dyn AppFs, lang: Lang) -> Vec<CatalogEntry> {
    let mut out = Vec::new();
    let Ok(ids) = installed_ids(fs) else {
        return out;
    };
    for id in ids {
        let Ok(bytes) = read_package_meta(fs, &id) else {
            continue;
        };
        let Ok(pkg) = appmanifest::parse_package(&bytes) else {
            continue;
        };
        if pkg.manifest.id != id {
            continue;
        }
        let icon = pkg.icon.as_ref().and_then(launcher_icon);
        out.push(CatalogEntry {
            manifest: pkg.manifest,
            icon,
        });
    }
    // By the name the launcher shows, accents and case ignored.
    out.sort_by_cached_key(|c| crate::format::search::fold(c.manifest.name_in(lang)));
    out
}

/// Offers each bundled package that is not installed yet and never overwrites one
/// that is (the user's copy wins). Returns how many were installed.
///
/// This is the stateless form (every call re-installs what was removed): use
/// [`seed_once`] on a volume that persists.
pub fn seed(fs: &mut dyn AppFs, bundled: &[&[u8]]) -> usize {
    let mut n = 0;
    for pkg in bundled {
        if install(fs, pkg).is_ok() {
            n += 1;
        }
    }
    n
}

/// The ids already offered to this volume (one per line).
const SEEDED: &str = "/apps/.seeded";
/// Largest marker file read back (64 ids of up to 32 bytes, with room to spare).
const SEEDED_MAX: usize = 4096;

fn read_seeded(fs: &mut dyn AppFs) -> Vec<String> {
    let mut buf = alloc::vec![0u8; SEEDED_MAX];
    let n = fs.read_at(SEEDED, 0, &mut buf).unwrap_or(0);
    let mut ids = Vec::new();
    for line in buf[..n].split(|&b| b == b'\n') {
        if let Ok(id) = core::str::from_utf8(line)
            && valid_id(id)
            && !ids.iter().any(|i| i == id)
        {
            ids.push(String::from(id));
        }
    }
    ids
}

fn write_seeded(fs: &mut dyn AppFs, ids: &[String]) -> Result<(), FsError> {
    let mut text = String::new();
    for id in ids.iter().take(MAX_APPS * 4) {
        text.push_str(id);
        text.push('\n');
    }
    match fs.create(SEEDED) {
        Ok(()) | Err(FsError::Exists) => {}
        Err(e) => return Err(e),
    }
    fs.set_len(SEEDED, 0)?;
    write_all(fs, SEEDED, text.as_bytes())
}

/// Replace the installed copy of `m.id` with `pkg` when that copy is older. The new bytes are
/// written to a temporary file first, so a failed write leaves the old package in place.
fn upgrade_if_older(fs: &mut dyn AppFs, pkg: &[u8], m: &Manifest) -> bool {
    // Only the manifest matters here: do not read the code of every installed app at every boot.
    let Ok(old) = read_package_meta(fs, &m.id) else {
        return false;
    };
    match check(&old) {
        Ok(o) if o.version < m.version => {}
        _ => return false,
    }
    let _ = fs.remove(TMP);
    if fs.create(TMP).is_err() {
        return false;
    }
    // The temporary copy is complete before the old package goes; `rename` does not replace.
    let ok = write_all(fs, TMP, pkg).is_ok()
        && fs.remove(&package_path(&m.id)).is_ok()
        && fs.rename(TMP, &package_path(&m.id)).is_ok();
    if !ok {
        let _ = fs.remove(TMP);
    }
    ok
}

/// Seeding for a volume that **persists**: each bundled package is offered once,
/// ever. A marker (`/apps/.seeded`, the ids already offered) keeps a package the
/// user removed from coming back at the next boot, while a package that is new in
/// this build of the OS (not in the marker) is still installed. A package that
/// fails for a transient reason (no space) is not marked, so it is retried.
/// A package already offered whose installed copy is **older** than the bundled one
/// (same id, lower version) is upgraded in place, so a disk formatted by an earlier
/// build gets the current names and fixes; a newer or equal copy is left alone.
/// Returns how many were installed or upgraded now.
pub fn seed_once(fs: &mut dyn AppFs, bundled: &[&[u8]]) -> usize {
    if fs.mkdir_all(APPS_DIR).is_err() {
        return 0;
    }
    let mut seen = read_seeded(fs);
    let before = seen.len();
    let mut installed = 0;
    for pkg in bundled {
        let Ok(m) = check(pkg) else { continue };
        if seen.contains(&m.id) {
            if upgrade_if_older(fs, pkg, &m) {
                installed += 1;
            }
            continue;
        }
        match install(fs, pkg) {
            Ok(_) => {
                installed += 1;
                seen.push(m.id);
            }
            // Already there (the user installed it first): it counts as offered.
            Err(InstallError::Duplicate) => seen.push(m.id),
            Err(_) => {}
        }
    }
    if seen.len() != before {
        let _ = write_seeded(fs, &seen);
    }
    installed
}

#[cfg(test)]
mod tests;
