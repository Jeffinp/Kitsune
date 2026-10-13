//! Files, arguments and standard input for a program.
//!
//! Every call acts with the rights of the user who started the program: the kernel checks them
//! against the file's owner, group and mode, and answers `-errno` (see [`error_text`]) when they do not
//! allow it. Relative paths start from the folder the program was started in.

use super::{syscall3, syscall5};
use kitsune_userabi::{self as abi, StatRec};

/// What a failed call returned (`-errno`, see `kitsune_userabi::errno`).
pub type Errno = i64;

fn check(r: i64) -> Result<u64, Errno> {
    if r < 0 { Err(r) } else { Ok(r as u64) }
}

/// A few words for an error, in English (programs write plain text to the terminal).
pub fn error_text(e: Errno) -> &'static str {
    use abi::errno::*;
    match e {
        ENOENT => "no such file or folder",
        EACCES => "permission denied",
        EEXIST => "already exists",
        ENOTDIR => "not a folder",
        EISDIR => "is a folder",
        EINVAL => "invalid argument",
        EMFILE => "too many open files",
        ENOSPC => "no space left",
        EFBIG => "file too big",
        ENAMETOOLONG => "name too long",
        ENOTEMPTY => "folder not empty",
        EBADF => "bad file descriptor",
        EFAULT => "bad address",
        EBUSY => "the volume is busy",
        ENODEV => "no such device",
        EAGAIN => "try again",
        _ => "i/o error",
    }
}

/// An open file; closed when dropped.
pub struct File {
    fd: i64,
}

impl File {
    /// Open an existing file for reading.
    pub fn open(path: &str) -> Result<File, Errno> {
        File::open_with(path, 0)
    }

    /// Create `path` (or empty it) and open it for writing.
    pub fn create(path: &str) -> Result<File, Errno> {
        File::open_with(path, abi::O_WRITE | abi::O_CREATE | abi::O_TRUNC)
    }

    /// Open `path` for appending, creating it when missing.
    pub fn append(path: &str) -> Result<File, Errno> {
        File::open_with(path, abi::O_WRITE | abi::O_CREATE | abi::O_APPEND)
    }

    /// Open with the `O_*` flags of `kitsune_userabi`.
    pub fn open_with(path: &str, flags: u64) -> Result<File, Errno> {
        // SAFETY: the pointer and length describe `path`.
        let fd = unsafe {
            syscall3(
                abi::SYS_OPEN,
                path.as_ptr() as u64,
                path.len() as u64,
                flags,
            )
        };
        check(fd).map(|fd| File { fd: fd as i64 })
    }

    /// Read up to `buf.len()` bytes; `Ok(0)` at the end of the file.
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, Errno> {
        // SAFETY: the pointer and length describe `buf`, which the kernel only writes to.
        let n = unsafe {
            syscall3(
                abi::SYS_READ,
                self.fd as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        check(n).map(|n| n as usize)
    }

    /// Write `data` at the current position (at the end, for a file opened to append).
    pub fn write(&mut self, data: &[u8]) -> Result<usize, Errno> {
        // SAFETY: the pointer and length describe `data`.
        let n = unsafe {
            syscall3(
                abi::SYS_WRITE,
                self.fd as u64,
                data.as_ptr() as u64,
                data.len() as u64,
            )
        };
        check(n).map(|n| n as usize)
    }

    /// Write all of `data`.
    pub fn write_all(&mut self, mut data: &[u8]) -> Result<(), Errno> {
        while !data.is_empty() {
            let n = self.write(data)?;
            if n == 0 {
                return Err(abi::errno::EIO);
            }
            data = &data[n..];
        }
        Ok(())
    }

    /// Move to `off` from `whence` (`SEEK_SET`, `SEEK_CUR` or `SEEK_END`); returns the new position.
    pub fn seek(&mut self, off: i64, whence: u64) -> Result<u64, Errno> {
        // SAFETY: no pointers.
        let p = unsafe { syscall3(abi::SYS_SEEK, self.fd as u64, off as u64, whence) };
        check(p)
    }
}

impl Drop for File {
    fn drop(&mut self) {
        // SAFETY: no pointers.
        unsafe { syscall3(abi::SYS_CLOSE, self.fd as u64, 0, 0) };
    }
}

/// Read standard input (what was piped to the program); `Ok(0)` at its end.
pub fn read_stdin(buf: &mut [u8]) -> Result<usize, Errno> {
    // SAFETY: the pointer and length describe `buf`.
    let n = unsafe { syscall3(abi::SYS_READ, 0, buf.as_mut_ptr() as u64, buf.len() as u64) };
    check(n).map(|n| n as usize)
}

/// Owner, mode, size and kind of `path`.
pub fn stat(path: &str) -> Result<StatRec, Errno> {
    let mut raw = [0u8; abi::STAT_LEN];
    // SAFETY: the pointers describe `path` and the 32-byte `raw`.
    let r = unsafe {
        syscall3(
            abi::SYS_STAT,
            path.as_ptr() as u64,
            path.len() as u64,
            raw.as_mut_ptr() as u64,
        )
    };
    check(r).map(|_| StatRec::from_bytes(&raw))
}

/// The `index`-th entry of folder `path`: its kind (`KIND_FILE` / `KIND_DIR`) and name, written into
/// `name`; `Ok(None)` past the last one.
pub fn read_dir<'a>(
    path: &str,
    index: usize,
    name: &'a mut [u8; 256],
) -> Result<Option<(u32, &'a [u8])>, Errno> {
    let mut rec = [0u8; 256];
    // SAFETY: the pointers describe `path` and the 256-byte `rec`.
    let r = unsafe {
        syscall5(
            abi::SYS_READDIR,
            path.as_ptr() as u64,
            path.len() as u64,
            index as u64,
            rec.as_mut_ptr() as u64,
            rec.len() as u64,
        )
    };
    let len = check(r)? as usize;
    if len == 0 {
        return Ok(None);
    }
    name[..len].copy_from_slice(&rec[1..1 + len]);
    Ok(Some((u32::from(rec[0]), &name[..len])))
}

/// Create the folder `path`.
pub fn mkdir(path: &str) -> Result<(), Errno> {
    // SAFETY: the pointer and length describe `path`.
    check(unsafe { syscall3(abi::SYS_MKDIR, path.as_ptr() as u64, path.len() as u64, 0) })
        .map(|_| ())
}

/// Move `path` to the trash (it can be put back from the file manager).
pub fn remove(path: &str) -> Result<(), Errno> {
    // SAFETY: the pointer and length describe `path`.
    check(unsafe { syscall3(abi::SYS_REMOVE, path.as_ptr() as u64, path.len() as u64, 0) })
        .map(|_| ())
}

/// Rename or move `from` to `to` (which must not exist).
pub fn rename(from: &str, to: &str) -> Result<(), Errno> {
    // SAFETY: the pointers and lengths describe `from` and `to`.
    check(unsafe {
        syscall5(
            abi::SYS_RENAME,
            from.as_ptr() as u64,
            from.len() as u64,
            to.as_ptr() as u64,
            to.len() as u64,
            0,
        )
    })
    .map(|_| ())
}

/// The arguments the program was started with and the folder it runs in.
pub struct Env {
    buf: [u8; abi::MAX_ARGS],
    len: usize,
}

impl Env {
    /// Ask the kernel for them.
    pub fn load() -> Env {
        let mut e = Env {
            buf: [0; abi::MAX_ARGS],
            len: 0,
        };
        // SAFETY: the pointer and length describe `e.buf`.
        let n = unsafe {
            syscall3(
                abi::SYS_GETARGS,
                e.buf.as_mut_ptr() as u64,
                e.buf.len() as u64,
                0,
            )
        };
        e.len = (n.max(0) as usize).min(e.buf.len());
        e
    }

    /// The folder the program was started in.
    pub fn cwd(&self) -> &str {
        abi::unpack_args(&self.buf[..self.len]).0
    }

    /// The arguments after the program's name.
    pub fn args(&self) -> abi::Args<'_> {
        abi::unpack_args(&self.buf[..self.len]).1
    }
}
