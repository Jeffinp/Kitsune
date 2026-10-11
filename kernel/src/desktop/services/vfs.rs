//! The desktop's one way to reach files: a small VFS over OJFS v3.
//!
//! Every consumer (file manager, image viewer, editor, terminal commands) goes
//! through this module; nothing in the desktop touches a filesystem image any
//! more. The logic (paths, names, copy, move, trash) is in `kitsune_core::vfs`,
//! unit-tested on the host; this file is the glue: which volume, the lock, the
//! clock.
//!
//! # API (stable)
//!
//! Paths are absolute byte strings (`b"/Documentos/a.txt"`, `b"/"` is the root);
//! the trash is `/.trash`, reached only through the `trash_*`/`restore` calls.
//! Names are UTF-8, 1..=255 bytes, no `/`. Every call returns
//! `Result<_, VfsError>`; `VfsError::message()` is a Portuguese (ASCII) sentence
//! for the user. Mutating calls are durable on `Ok` when the volume is the disk.
//!
//! | Call | Does |
//! |---|---|
//! | `read_file(path)` / `read_range(path, off, len)` / `write_file(path, data)` / `append(path, data)` | whole-file read, a slice of it, atomic create-or-replace, append (creates) |
//! | `list(dir)` -> `Vec<Entry>` | name, kind, size, mtime (storage order; `.trash` hidden) |
//! | `stat(path)` -> `Info` / `exists(path)` / `statfs()` -> `Usage` | metadata, free space |
//! | `mkdir(path)` / `new_file(dir, name)` / `new_folder(dir, name)` | create (name is validated, returns the path) |
//! | `rename(path, new_name)` / `rename_path(from, to)` | same-folder rename / explicit move (destination must be free) |
//! | `remove(path)` | move to the trash (a folder takes its contents) |
//! | `purge(path)` | delete for good |
//! | `trash_list()` / `restore(id)` / `trash_purge(id)` / `empty_trash()` | trash management (`id` is `TrashItem::id`) |
//! | `move_to(sources, dest_dir)` | rename into a folder, `name (2).ext` on collision; instant |
//! | `copy_plan` + `copy_step` + `copy_abort` | recursive copy in bounded steps (progress, cancel) |
//! | `copy(sources, dest_dir)` | the same, run to completion (terminal `cp`) |
//! | `generation()` | bumps on every mutation: cheap "did the disk change?" for views |
//! | `volume()` / `notice()` | `Disk` or `Memory`, and the one-line warning to show when it is memory |
//!
//! # Volumes
//!
//! `storage::state() == V3` -> the mounted ATA volume. Anything else (a 64 KiB
//! disk, no disk, an unknown or failed one) -> a ~4 MiB `Fs3` on a `RamDisk`,
//! built on first use: files live only in memory, which [`notice`] says once.
//! A readable OJFS v2 image on a too-small disk is imported into that RAM volume
//! (`migrate_v2`) so the old files stay visible; otherwise it starts with the
//! welcome files (`vfs::seed_welcome`, the only seed). The disk itself is never
//! written on this path.

#![allow(dead_code)] // part of the API is for the editor/shell fronts

use crate::storage;
use crate::sync::YieldMutex;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use kitsune_core::blockdev::RamDisk;
use kitsune_core::fs3::{self, FormatOptions, Fs3};
use kitsune_core::vfs as core_vfs;

#[allow(unused_imports)]
pub use core_vfs::{
    Backend, CopyJob, Entry, EntryKind, Info, MAX_NAME, MoveReport, Progress, TrashItem, Usage,
    VfsError, base_name, is_root, join, parent, trim_name, validate_name,
};

/// Result of this module's calls.
pub type Result<T> = core::result::Result<T, VfsError>;

/// Size of the in-memory fallback volume.
const RAM_BYTES: u64 = 4 * 1024 * 1024;

/// The fallback volume (built lazily by [`with`]).
static RAMFS: YieldMutex<Option<Fs3<RamDisk>>> = YieldMutex::new(None);
static RAM_TRIED: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU32 = AtomicU32::new(1);

/// Which volume the desktop is on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Volume {
    /// The OJFS v3 disk: files persist.
    Disk,
    /// The RAM volume: files are lost at power-off.
    Memory,
}

/// Disk (v3 mounted) or memory.
pub fn volume() -> Volume {
    if storage::is_v3() {
        Volume::Disk
    } else {
        Volume::Memory
    }
}

/// The warning to show (once) when files live only in memory; `None` on a disk.
pub fn notice() -> Option<&'static str> {
    if volume() == Volume::Disk {
        return None;
    }
    Some(match storage::state() {
        storage::State::TooSmall => kitsune_core::t!("files.vol.too_small"),
        storage::State::NoDisk => kitsune_core::t!("files.vol.none"),
        storage::State::Unknown => kitsune_core::t!("files.vol.unknown"),
        _ => kitsune_core::t!("files.vol.failed"),
    })
}

/// A counter that changes whenever a call here modified the filesystem.
pub fn generation() -> u32 {
    GENERATION.load(Ordering::Relaxed)
}

fn touched() {
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Record that something outside this module (an app writing through the app
/// filesystem) changed the volume, so views that poll [`generation`] reload.
pub fn touch() {
    touched();
}

fn now() -> u64 {
    storage::now()
}

/// Build the RAM volume: import a readable v2 image from a too-small disk, else
/// format it and seed the welcome files. Runs once.
fn build_ram() -> Option<Fs3<RamDisk>> {
    let sectors = RAM_BYTES / 512;
    let t = now();
    let mut uuid = [0u8; 16];
    uuid[..8].copy_from_slice(&t.to_le_bytes());
    uuid[8..].copy_from_slice(&crate::io::rdtsc().to_le_bytes());
    let opts = FormatOptions::new(uuid, t);

    let mut dev = RamDisk::new(sectors);
    let mut imported = false;
    if storage::state() == storage::State::TooSmall
        && let Some(mut disk) = crate::ata::AtaDisk::open()
        && let Ok(img) = fs3::read_v2_image(&mut disk)
        && let Ok(rep) = fs3::migrate_v2(&mut dev, &img, &opts)
    {
        imported = true;
        crate::serial_println!(
            "vfs: imported the OJFS v2 disk into the RAM volume ({} files, {} dirs)",
            rep.files,
            rep.dirs
        );
    }
    let mut fs = if imported {
        Fs3::mount(dev).ok()?
    } else {
        let mut fs = Fs3::format(dev, &opts).ok()?;
        let _ = core_vfs::seed_welcome(&mut fs, t);
        fs
    };
    let _ = fs.sync();
    Some(fs)
}

fn ensure_ram() {
    if RAM_TRIED.swap(true, Ordering::AcqRel) {
        return;
    }
    let built = build_ram();
    match built {
        Some(fs) => {
            if let Ok(mut slot) = RAMFS.lock() {
                *slot = Some(fs);
            }
            crate::serial_println!(
                "vfs: {} ({} MiB RAM volume)",
                notice().unwrap_or("memory volume"),
                RAM_BYTES / (1024 * 1024)
            );
        }
        None => crate::serial_println!("vfs: cannot build the RAM volume"),
    }
}

/// Run `f` on the active volume with full rights: for the system's own files (settings, logs,
/// the app store, the account database), never for what a user asked for.
fn with_sys<R>(f: impl FnOnce(&mut dyn Backend) -> R) -> Result<R> {
    if storage::is_v3() {
        return storage::with_fs(|fs| f(fs)).ok_or(VfsError::Busy);
    }
    ensure_ram();
    let mut guard = RAMFS.lock().map_err(|_| VfsError::Busy)?;
    match guard.as_mut() {
        Some(fs) => Ok(f(fs)),
        None => Err(VfsError::Unavailable),
    }
}

/// Run `f` on the active volume as the signed-in user: every operation goes through
/// [`Secured`](kitsune_core::secured::Secured) with the session's credentials. Before an account
/// exists (early boot) there is no session and the call has full rights.
fn with_user<R>(f: impl FnOnce(&mut dyn Backend) -> R) -> Result<R> {
    // Accounts exist and nobody is signed in: nothing a user asked for may touch the files.
    if super::accounts::locked_out() {
        return Err(VfsError::PermissionDenied);
    }
    let cred = super::accounts::cred();
    with_sys(|be| match cred {
        Some(c) => {
            let mut s =
                kitsune_core::secured::Secured::new(be, c, kitsune_core::secured::DEFAULT_UMASK);
            f(&mut s)
        }
        None => f(be),
    })
}

/// Run `f` on the active volume as `cred` (`None`: full rights, like [`with_user`] before any account
/// exists). For a program that outlives the session it was started in: it keeps acting as the user
/// who ran it. The volume being unavailable is an `Err`.
pub(crate) fn as_cred<R>(
    cred: Option<kitsune_core::perm::Cred>,
    f: impl FnOnce(&mut dyn Backend) -> R,
) -> Result<R> {
    with_sys(|be| match cred {
        Some(c) => {
            let mut s =
                kitsune_core::secured::Secured::new(be, c, kitsune_core::secured::DEFAULT_UMASK);
            f(&mut s)
        }
        None => f(be),
    })
}

fn mut_sys<R>(f: impl FnOnce(&mut dyn Backend) -> R) -> Result<R> {
    let r = with_sys(f);
    touched();
    r
}

fn mut_user<R>(f: impl FnOnce(&mut dyn Backend) -> R) -> Result<R> {
    let r = with_user(f);
    touched();
    r
}

macro_rules! vfs_api {
    ($with:ident, $with_mut:ident) => {
    /// Run `f` on the active volume without marking it modified (for read-only
    /// walks such as the file manager's reload). `f` gets the [`Backend`].
    pub fn with_backend<R>(f: impl FnOnce(&mut dyn Backend) -> R) -> Result<R> {
        $with(f)
    }

    pub fn read_file(path: &[u8]) -> Result<Vec<u8>> {
        $with(|b| b.read_file(path))?
    }

    pub fn write_file(path: &[u8], data: &[u8]) -> Result<()> {
        $with_mut(|b| b.write_file(path, data, now()))?
    }

    pub fn append(path: &[u8], data: &[u8]) -> Result<()> {
        $with_mut(|b| {
            if b.stat(path).is_err() {
                b.create(path, now())?;
            }
            b.append(path, data, now())
        })?
    }

    /// Read up to `len` bytes of `path` starting at `off` (a prefix of a big file).
    pub fn read_range(path: &[u8], off: u64, len: usize) -> Result<Vec<u8>> {
        $with(|b| {
            let mut buf = alloc::vec![0u8; len];
            let n = b.read_at(path, off, &mut buf)?;
            buf.truncate(n);
            Ok(buf)
        })?
    }

    /// A name based on `base` that is free in folder `dir` (`Nova pasta (2)`).
    pub fn unique_name_in(dir: &[u8], base: &[u8]) -> Vec<u8> {
        $with(|b| core_vfs::unique_name(base, |n| core_vfs::exists(b, &join(dir, n))))
            .unwrap_or_else(|_| base.to_vec())
    }

    pub fn list(dir: &[u8]) -> Result<Vec<Entry>> {
        $with(|b| core_vfs::list(b, dir))?
    }

    pub fn stat(path: &[u8]) -> Result<Info> {
        $with(|b| b.stat(path))?
    }

    pub fn exists(path: &[u8]) -> bool {
        $with(|b| core_vfs::exists(b, path)).unwrap_or(false)
    }

    pub fn statfs() -> Usage {
        $with(|b| b.usage()).unwrap_or_default()
    }

    pub fn mkdir(path: &[u8]) -> Result<()> {
        $with_mut(|b| b.mkdir(path, now()))?
    }

    pub fn new_file(dir: &[u8], name: &[u8]) -> Result<Vec<u8>> {
        $with_mut(|b| core_vfs::new_file(b, dir, name, now()))?
    }

    pub fn new_folder(dir: &[u8], name: &[u8]) -> Result<Vec<u8>> {
        $with_mut(|b| core_vfs::new_folder(b, dir, name, now()))?
    }

    pub fn rename(path: &[u8], new_name: &[u8]) -> Result<Vec<u8>> {
        $with_mut(|b| core_vfs::rename_in(b, path, new_name, now()))?
    }

    pub fn rename_path(from: &[u8], to: &[u8]) -> Result<()> {
        $with_mut(|b| b.rename(from, to, now()))?
    }

    pub fn remove(path: &[u8]) -> Result<()> {
        $with_mut(|b| core_vfs::remove(b, path, now()))?
    }

    pub fn purge(path: &[u8]) -> Result<()> {
        $with_mut(|b| core_vfs::purge(b, path))?
    }

    pub fn trash_list() -> Result<Vec<TrashItem>> {
        $with(|b| b.trash_list())?
    }

    pub fn restore(id: &[u8]) -> Result<Vec<u8>> {
        $with_mut(|b| b.trash_restore(id, now()))?
    }

    pub fn trash_purge(id: &[u8]) -> Result<()> {
        $with_mut(|b| b.trash_purge(id))?
    }

    pub fn empty_trash() -> Result<()> {
        $with_mut(|b| b.empty_trash())?
    }

    pub fn move_to(sources: &[Vec<u8>], dest: &[u8]) -> MoveReport {
        match $with_mut(|b| core_vfs::move_to(b, sources, dest, now())) {
            Ok(r) => r,
            Err(e) => MoveReport {
                moved: Vec::new(),
                error: Some(e),
            },
        }
    }

    /// Plan a recursive copy of `sources` into the folder `dest`.
    pub fn copy_plan(sources: &[Vec<u8>], dest: &[u8]) -> Result<CopyJob> {
        $with(|b| CopyJob::plan(b, sources, dest))?
    }

    /// Copy up to `budget` bytes. An error ends the job (the partial file is removed).
    pub fn copy_step(job: &mut CopyJob, budget: usize) -> Result<Progress> {
        $with_mut(|b| job.step(b, budget, now()))?
    }

    /// Cancel: delete the half-written file; finished copies stay.
    pub fn copy_abort(job: &mut CopyJob) {
        let _ = $with_mut(|b| job.abort(b));
    }

    /// Copy `sources` into `dest` and wait for it; returns the new paths.
    pub fn copy(sources: &[Vec<u8>], dest: &[u8]) -> Result<Vec<Vec<u8>>> {
        let mut job = copy_plan(sources, dest)?;
        while copy_step(&mut job, core_vfs::COPY_CHUNK * 4)? == Progress::Running {}
        Ok(job.results().to_vec())
    }
    };
}

vfs_api!(with_user, mut_user);

/// The same calls with full rights, for the system's own files (settings, logs, the app store, the
/// account database): the user's permissions do not apply to them.
pub mod root {
    pub use super::touch;
    use super::*;
    vfs_api!(with_sys, mut_sys);
}
