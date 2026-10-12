//! Files and arguments for ring 3 programs: what both sides of `int 0x80` agree on.
//!
//! Numbers 0–6 (exit, write, yield, getpid, sleep, uptime, brk) and 16–21 (the WebAssembly host,
//! `kitsune_core::appabi::remote`) are documented where they are served; this crate owns 32–42 (files
//! and arguments) and 48–51 (a window).
//! A call is `rax` = number, `rdi`, `rsi`, `rdx`, `r10`, `r8` = arguments, result in `rax`
//! (negative = `-errno`, see [`errno`]). Pointers are user addresses; the kernel copies through the
//! process's page tables and never trusts them.
//!
//! Paths may be absolute or relative to the working directory the program was started in (see
//! [`pack_args`]); the program resolves them with [`resolve`], the kernel only ever sees absolute ones.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
use alloc::{string::String, vec::Vec};

pub const SYS_OPEN: u64 = 32;
pub const SYS_CLOSE: u64 = 33;
pub const SYS_READ: u64 = 34;
pub const SYS_WRITE: u64 = 35;
pub const SYS_SEEK: u64 = 36;
pub const SYS_STAT: u64 = 37;
pub const SYS_READDIR: u64 = 38;
pub const SYS_MKDIR: u64 = 39;
pub const SYS_REMOVE: u64 = 40;
pub const SYS_RENAME: u64 = 41;
pub const SYS_GETARGS: u64 = 42;

pub const SYS_WIN_OPEN: u64 = 48;
pub const SYS_WIN_PRESENT: u64 = 49;
pub const SYS_WIN_EVENT: u64 = 50;
pub const SYS_WIN_TITLE: u64 = 51;

/// Largest window content, in pixels (the biggest the desktop shows).
pub const WIN_MAX_W: u32 = 1280;
pub const WIN_MAX_H: u32 = 800;
/// Longest window title, in bytes.
pub const WIN_MAX_TITLE: usize = 64;

/// Kinds of [`Event`].
pub const EV_KEY: u32 = 1;
pub const EV_TEXT: u32 = 2;
pub const EV_POINTER: u32 = 3;
pub const EV_RESIZE: u32 = 4;
pub const EV_CLOSE: u32 = 5;

/// What [`SYS_WIN_EVENT`] writes: 16 bytes, little endian.
///
/// | kind | `a` | `b` | `c` |
/// |---|---|---|---|
/// | `EV_KEY` | key code (ASCII, 10 Enter, 27 Esc, 8 Backspace, 127 Delete, 0x100.. arrows) | modifiers (bit 0 Shift, 1 Ctrl, 2 Alt) | |
/// | `EV_TEXT` | the Unicode character typed | | |
/// | `EV_POINTER` | x in the content | y | buttons (bit 0 left, 1 right) |
/// | `EV_RESIZE` | new content width | height | |
/// | `EV_CLOSE` | | | the window was asked to close: finish |
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Event {
    pub kind: u32,
    pub a: i32,
    pub b: i32,
    pub c: i32,
}

/// Size of an [`Event`] on the wire.
pub const EVENT_LEN: usize = 16;

impl Event {
    pub fn to_bytes(&self) -> [u8; EVENT_LEN] {
        let mut o = [0u8; EVENT_LEN];
        o[0..4].copy_from_slice(&self.kind.to_le_bytes());
        o[4..8].copy_from_slice(&self.a.to_le_bytes());
        o[8..12].copy_from_slice(&self.b.to_le_bytes());
        o[12..16].copy_from_slice(&self.c.to_le_bytes());
        o
    }

    pub fn from_bytes(b: &[u8; EVENT_LEN]) -> Event {
        let w = |i: usize| [b[i], b[i + 1], b[i + 2], b[i + 3]];
        Event {
            kind: u32::from_le_bytes(w(0)),
            a: i32::from_le_bytes(w(4)),
            b: i32::from_le_bytes(w(8)),
            c: i32::from_le_bytes(w(12)),
        }
    }
}

/// `open` flags.
pub const O_WRITE: u64 = 1;
pub const O_CREATE: u64 = 2;
pub const O_TRUNC: u64 = 4;
pub const O_APPEND: u64 = 8;

/// `seek` origins.
pub const SEEK_SET: u64 = 0;
pub const SEEK_CUR: u64 = 1;
pub const SEEK_END: u64 = 2;

/// Descriptors: 0 is the standard input (what was piped to the program), 1 and 2 the output, files
/// start at [`FIRST_FD`].
pub const FIRST_FD: i64 = 3;
/// Open files per process.
pub const MAX_FDS: usize = 16;
/// Longest path, in bytes.
pub const MAX_PATH: usize = 1024;
/// Most bytes one `read` or `write` moves.
pub const MAX_IO: usize = 64 * 1024;
/// Most bytes of arguments (the working directory included).
pub const MAX_ARGS: usize = 4096;

/// Negative results.
pub mod errno {
    pub const EIO: i64 = -5;
    pub const EBADF: i64 = -9;
    pub const EBUSY: i64 = -16;
    pub const EACCES: i64 = -13;
    pub const EFAULT: i64 = -14;
    pub const EEXIST: i64 = -17;
    pub const ENOTDIR: i64 = -20;
    pub const EISDIR: i64 = -21;
    pub const EINVAL: i64 = -22;
    pub const EMFILE: i64 = -24;
    pub const EFBIG: i64 = -27;
    pub const ENOSPC: i64 = -28;
    pub const ENAMETOOLONG: i64 = -36;
    pub const ENOTEMPTY: i64 = -39;
    pub const ENOENT: i64 = -2;
}

/// What [`SYS_STAT`] writes: 32 bytes, little endian.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatRec {
    /// 1 file, 2 directory.
    pub kind: u32,
    pub mode: u32,
    pub size: u64,
    /// Seconds since the Unix epoch (UTC).
    pub mtime: u64,
    pub uid: u32,
    pub gid: u32,
}

pub const KIND_FILE: u32 = 1;
pub const KIND_DIR: u32 = 2;
/// Size of a [`StatRec`] on the wire.
pub const STAT_LEN: usize = 32;

impl StatRec {
    pub fn to_bytes(&self) -> [u8; STAT_LEN] {
        let mut b = [0u8; STAT_LEN];
        b[0..4].copy_from_slice(&self.kind.to_le_bytes());
        b[4..8].copy_from_slice(&self.mode.to_le_bytes());
        b[8..16].copy_from_slice(&self.size.to_le_bytes());
        b[16..24].copy_from_slice(&self.mtime.to_le_bytes());
        b[24..28].copy_from_slice(&self.uid.to_le_bytes());
        b[28..32].copy_from_slice(&self.gid.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8; STAT_LEN]) -> StatRec {
        let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let u64_at = |i: usize| {
            u64::from_le_bytes([
                b[i],
                b[i + 1],
                b[i + 2],
                b[i + 3],
                b[i + 4],
                b[i + 5],
                b[i + 6],
                b[i + 7],
            ])
        };
        StatRec {
            kind: u32_at(0),
            mode: u32_at(4),
            size: u64_at(8),
            mtime: u64_at(16),
            uid: u32_at(24),
            gid: u32_at(28),
        }
    }
}

/// The arguments blob [`SYS_GETARGS`] returns: the working directory, then each argument, every one
/// followed by a NUL byte. `None` if it would exceed [`MAX_ARGS`] or an item holds a NUL.
#[cfg(feature = "alloc")]
pub fn pack_args(cwd: &str, args: &[String]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for item in core::iter::once(cwd).chain(args.iter().map(String::as_str)) {
        if item.bytes().any(|b| b == 0) {
            return None;
        }
        out.extend_from_slice(item.as_bytes());
        out.push(0);
    }
    (out.len() <= MAX_ARGS).then_some(out)
}

/// Splits a blob made by [`pack_args`]: the working directory and an iterator over the arguments.
/// Items that are not UTF-8 read as empty.
pub fn unpack_args(blob: &[u8]) -> (&str, Args<'_>) {
    let end = blob.iter().position(|&b| b == 0).unwrap_or(blob.len());
    let cwd = core::str::from_utf8(&blob[..end]).unwrap_or("/");
    let rest = blob.get(end + 1..).unwrap_or(&[]);
    (cwd, Args { rest })
}

/// The arguments of a program, in order.
#[derive(Clone, Debug)]
pub struct Args<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for Args<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.rest.is_empty() {
            return None;
        }
        let end = self
            .rest
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.rest.len());
        let item = &self.rest[..end];
        self.rest = self.rest.get(end + 1..).unwrap_or(&[]);
        Some(core::str::from_utf8(item).unwrap_or(""))
    }
}

/// Appends `bytes` to `out[..*len]`.
fn push(out: &mut [u8; MAX_PATH], len: &mut usize, bytes: &[u8]) -> Option<()> {
    let end = len.checked_add(bytes.len())?;
    out.get_mut(*len..end)?.copy_from_slice(bytes);
    *len = end;
    Some(())
}

/// An absolute, normalised path made of `cwd` and `path`: `.` and empty parts vanish, `..` goes up
/// (never above the root). Written into `out`; returns its length, or `None` when it does not fit
/// or either holds a NUL. An absolute `path` ignores `cwd`; a relative `cwd` counts from the root.
pub fn resolve(cwd: &str, path: &str, out: &mut [u8; MAX_PATH]) -> Option<usize> {
    if path.bytes().any(|b| b == 0) || cwd.bytes().any(|b| b == 0) {
        return None;
    }
    let mut len = 0usize;
    let sources: [&str; 2] = if path.starts_with('/') {
        ["", path]
    } else {
        [cwd, path]
    };
    for src in sources {
        for part in src.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    // Drop the last component.
                    while len > 0 {
                        len -= 1;
                        if out[len] == b'/' {
                            break;
                        }
                    }
                }
                name => {
                    push(out, &mut len, b"/")?;
                    push(out, &mut len, name.as_bytes())?;
                }
            }
        }
    }
    if len == 0 {
        push(out, &mut len, b"/")?;
    }
    Some(len)
}

/// [`resolve`] into a `String` (the kernel side).
#[cfg(feature = "alloc")]
pub fn resolve_string(cwd: &str, path: &str) -> Option<String> {
    let mut buf = [0u8; MAX_PATH];
    let n = resolve(cwd, path, &mut buf)?;
    core::str::from_utf8(&buf[..n]).ok().map(String::from)
}

#[cfg(all(test, feature = "alloc"))]
mod tests;
