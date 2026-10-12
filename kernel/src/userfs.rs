//! `procfsd`: the thread that does the file and window requests of ring 3 processes.
//!
//! A process asks for a file operation with `int 0x80` (calls 32–42, `kitsune_userabi`); the call only
//! *queues* the request (the kernel is in an interrupt there, where a disk transfer and a lock that
//! somebody else holds cannot be waited for) and blocks the process. This thread takes the request,
//! runs it on the volume **as the user who started the program** (`vfs::as_cred`: every operation goes
//! through `Secured` with that user's credentials, so a program can read and write exactly what
//! its user can and nothing else), moves the bytes to and from the process's memory page by page, and
//! answers; the process's call is then restarted and returns the result.

use crate::desktop::vfs;
use crate::userproc::{self, Fd, FsReq};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use kitsune_core::perm::Cred;
use kitsune_core::vfs::{Backend, EntryKind, VfsError};
use kitsune_userabi::{self as abi, errno};

static TID: AtomicUsize = AtomicUsize::new(usize::MAX);

/// Wake the thread (a request was queued).
pub fn wake() {
    let t = TID.load(Ordering::Acquire);
    if t != usize::MAX {
        crate::sched::wake(t);
    }
}

/// Let the next voluntary switch go to the thread: the process that queued a request is about to block.
pub fn hand_off() {
    let t = TID.load(Ordering::Acquire);
    if t != usize::MAX {
        crate::sched::hand_off(t);
    }
}

/// The thread's entry point.
pub extern "C" fn worker() -> ! {
    TID.store(crate::sched::current(), Ordering::Release);
    loop {
        match userproc::fs_take() {
            Some((pid, req, cred)) => {
                let v = serve(pid, req, cred);
                userproc::fs_done(pid, v);
            }
            None => crate::sched::block(crate::sched::FOREVER, || !userproc::fs_pending()),
        }
    }
}

/// The result a `VfsError` becomes.
fn errno_of(e: VfsError) -> i64 {
    match e {
        VfsError::NotFound => errno::ENOENT,
        VfsError::Exists => errno::EEXIST,
        VfsError::NotDir => errno::ENOTDIR,
        VfsError::IsDir => errno::EISDIR,
        VfsError::NotEmpty => errno::ENOTEMPTY,
        VfsError::InvalidName
        | VfsError::InvalidPath
        | VfsError::Reserved
        | VfsError::InvalidMove
        | VfsError::Cancelled => errno::EINVAL,
        VfsError::NameTooLong => errno::ENAMETOOLONG,
        VfsError::NoSpace | VfsError::NoInodes => errno::ENOSPC,
        VfsError::TooBig => errno::EFBIG,
        VfsError::Busy | VfsError::Unavailable => errno::EBUSY,
        VfsError::Io | VfsError::Corrupt => errno::EIO,
        VfsError::PermissionDenied => errno::EACCES,
    }
}

fn now() -> u64 {
    crate::storage::now()
}

/// Run `f` on the volume as `cred`; the volume being busy or missing is an error result.
fn on_volume<R>(
    cred: &Option<Cred>,
    f: impl FnOnce(&mut dyn Backend) -> Result<R, VfsError>,
) -> Result<R, i64> {
    match vfs::as_cred(cred.clone(), f) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) | Err(e) => Err(errno_of(e)),
    }
}

fn serve(pid: u32, req: FsReq, cred: Option<Cred>) -> i64 {
    match run(pid, req, &cred) {
        Ok(v) | Err(v) => v,
    }
}

fn run(pid: u32, req: FsReq, cred: &Option<Cred>) -> Result<i64, i64> {
    match req {
        FsReq::Open { path, flags } => {
            let write = flags & abi::O_WRITE != 0;
            let (create, trunc) = (flags & abi::O_CREATE != 0, flags & abi::O_TRUNC != 0);
            let size = on_volume(cred, |b| match b.stat(&path) {
                Ok(info) if info.kind == EntryKind::Dir => Err(VfsError::IsDir),
                Ok(info) => {
                    // Ask the permission now, not at the first byte: a read of nothing / a write of nothing.
                    if write && trunc {
                        b.truncate(&path, 0, now())?;
                        Ok(0)
                    } else if write {
                        b.write_at(&path, 0, &[], now())?;
                        Ok(info.size)
                    } else {
                        b.read_at(&path, 0, &mut [])?;
                        Ok(info.size)
                    }
                }
                Err(VfsError::NotFound) if write && create => {
                    b.create(&path, now())?;
                    Ok(0)
                }
                Err(e) => Err(e),
            })?;
            vfs::touch();
            let append = flags & abi::O_APPEND != 0;
            Ok(userproc::fs_new_fd(
                pid,
                Fd {
                    path,
                    pos: if append { size } else { 0 },
                    write,
                    append,
                },
            ))
        }
        FsReq::Read { fd, ptr, len } => {
            let mut f = userproc::fs_fd(pid, fd).ok_or(errno::EBADF)?;
            let mut buf = alloc::vec![0u8; len];
            let n = on_volume(cred, |b| b.read_at(&f.path, f.pos, &mut buf))?;
            if !userproc::host_write(pid, ptr, &buf[..n]) {
                return Err(errno::EFAULT);
            }
            f.pos += n as u64;
            userproc::fs_set_fd(pid, fd, f);
            Ok(n as i64)
        }
        FsReq::Write { fd, ptr, len } => {
            let mut f = userproc::fs_fd(pid, fd).ok_or(errno::EBADF)?;
            if !f.write {
                return Err(errno::EBADF);
            }
            let data = userproc::host_read(pid, ptr, len).ok_or(errno::EFAULT)?;
            on_volume(cred, |b| {
                if f.append {
                    b.append(&f.path, &data, now())
                } else {
                    b.write_at(&f.path, f.pos, &data, now())
                }
            })?;
            vfs::touch();
            if f.append {
                let end = on_volume(cred, |b| b.stat(&f.path).map(|i| i.size))?;
                f.pos = end;
            } else {
                f.pos += data.len() as u64;
            }
            userproc::fs_set_fd(pid, fd, f);
            Ok(data.len() as i64)
        }
        FsReq::Seek { fd, off, whence } => {
            let mut f = userproc::fs_fd(pid, fd).ok_or(errno::EBADF)?;
            let base = match whence {
                abi::SEEK_SET => 0i64,
                abi::SEEK_CUR => f.pos as i64,
                abi::SEEK_END => on_volume(cred, |b| b.stat(&f.path).map(|i| i.size))? as i64,
                _ => return Err(errno::EINVAL),
            };
            let pos = base
                .checked_add(off)
                .filter(|p| *p >= 0)
                .ok_or(errno::EINVAL)?;
            f.pos = pos as u64;
            userproc::fs_set_fd(pid, fd, f);
            Ok(pos)
        }
        FsReq::Stat { path, out } => {
            let info = on_volume(cred, |b| b.stat(&path))?;
            let rec = abi::StatRec {
                kind: if info.kind == EntryKind::Dir {
                    abi::KIND_DIR
                } else {
                    abi::KIND_FILE
                },
                mode: u32::from(info.mode),
                size: info.size,
                mtime: info.mtime,
                uid: info.uid,
                gid: info.gid,
            };
            if userproc::host_write(pid, out, &rec.to_bytes()) {
                Ok(0)
            } else {
                Err(errno::EFAULT)
            }
        }
        FsReq::ReadDir {
            path,
            index,
            out,
            cap,
        } => {
            if !(2..=256).contains(&cap) {
                return Err(errno::EINVAL);
            }
            let entries = on_volume(cred, |b| kitsune_core::vfs::list(b, &path))?;
            let Some(e) = entries.get(index as usize) else {
                return Ok(0);
            };
            if e.name.len() + 1 > cap {
                return Err(errno::EINVAL);
            }
            let mut rec = Vec::with_capacity(e.name.len() + 1);
            rec.push(if e.kind == EntryKind::Dir {
                abi::KIND_DIR as u8
            } else {
                abi::KIND_FILE as u8
            });
            rec.extend_from_slice(&e.name);
            if userproc::host_write(pid, out, &rec) {
                Ok(e.name.len() as i64)
            } else {
                Err(errno::EFAULT)
            }
        }
        FsReq::Mkdir { path } => {
            on_volume(cred, |b| b.mkdir(&path, now()))?;
            vfs::touch();
            Ok(0)
        }
        // To the trash, like every other way a user deletes something: it can be put back.
        FsReq::Remove { path } => {
            on_volume(cred, |b| kitsune_core::vfs::remove(b, &path, now()))?;
            vfs::touch();
            Ok(0)
        }
        FsReq::WinOpen { w, h, title } => {
            let (name, _) = userproc::win_of(pid).ok_or(errno::EBADF)?;
            let title = core::str::from_utf8(&title).map_err(|_| errno::EINVAL)?;
            let handle =
                crate::wasm::launch_native(pid, &name, title, w, h).map_err(|_| errno::ENOSPC)?;
            userproc::set_win(pid, handle);
            Ok(0)
        }
        FsReq::WinPresent { ptr, w, h } => {
            let (_, handle) = userproc::win_of(pid).ok_or(errno::EBADF)?;
            let n = (w as usize) * (h as usize) * 4;
            let rgba = userproc::host_read(pid, ptr, n).ok_or(errno::EFAULT)?;
            if crate::wasm::native_present(handle, w, h, &rgba) {
                Ok(0)
            } else {
                Err(errno::EINVAL)
            }
        }
        FsReq::WinTitle { title } => {
            let (_, handle) = userproc::win_of(pid).ok_or(errno::EBADF)?;
            crate::wasm::native_title(handle, core::str::from_utf8(&title).unwrap_or(""));
            Ok(0)
        }
        FsReq::Rename { from, to } => {
            on_volume(cred, |b| b.rename(&from, &to, now()))?;
            vfs::touch();
            Ok(0)
        }
    }
}
