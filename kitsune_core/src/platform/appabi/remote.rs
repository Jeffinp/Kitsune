//! The wire format between a ring 3 app host (`user-apps/wasmhost`) and the kernel.
//!
//! An app can run in two places: inside the kernel (`wasmi` linked into it) or in a **user process**
//! that runs `wasmi` and forwards every `osj.*` call to the kernel. The kernel keeps everything that
//! needs authority (files, network, clipboard, the surface the app draws on); the process holds only
//! the interpreter, the app's linear memory and its fuel. This module is the contract between the two
//! sides: which host functions exist and their signatures, the system-call numbers, and the layout of
//! the three records that cross the boundary.
//!
//! Flow (all calls are `int 0x80`, see `kernel/src/userproc.rs`):
//!
//! 1. The host asks for the module bytes ([`SYS_HOST_MODULE_LEN`], [`SYS_HOST_MODULE_READ`]).
//! 2. It loops on [`SYS_HOST_JOB_WAIT`], which blocks until the kernel posts a [`Job`]: `JOB_INIT`
//!    (instantiate with a memory cap and fuel) or `JOB_CALL` (run one export with arguments).
//! 3. While a job runs, each imported function the guest calls becomes a [`SYS_HOST_CALL`] with a
//!    [`HostCall`] record; the kernel serves it (reading and writing the guest's memory through the
//!    process's own page tables, never trusting a pointer) and returns the result.
//! 4. The host ends the job with [`SYS_HOST_JOB_DONE`] and a [`Done`] record.
//!
//! All records are `repr(C)` and little endian; the kernel copies them with `copy_in`/`copy_out`.

/// A WebAssembly value type of a host function signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    I32,
    I64,
}

/// One function of the `osj` import module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostFn {
    pub name: &'static str,
    pub params: &'static [Ty],
    pub results: &'static [Ty],
}

use Ty::{I32, I64};

/// Every `osj.*` function, indexed by its call id (the position in this table is the id on the wire).
pub const HOST_FNS: &[HostFn] = &[
    HostFn {
        name: "set_title",
        params: &[I32, I32],
        results: &[I32],
    },
    HostFn {
        name: "get_size",
        params: &[],
        results: &[I64],
    },
    HostFn {
        name: "request_redraw",
        params: &[],
        results: &[],
    },
    HostFn {
        name: "fill_rect",
        params: &[I32, I32, I32, I32, I32],
        results: &[],
    },
    HostFn {
        name: "draw_text",
        params: &[I32, I32, I32, I32, I32, I32],
        results: &[I32],
    },
    HostFn {
        name: "lang",
        params: &[],
        results: &[I32],
    },
    HostFn {
        name: "blit_rgba",
        params: &[I32, I32, I32, I32, I32],
        results: &[I32],
    },
    HostFn {
        name: "draw_image_png",
        params: &[I32, I32, I32, I32],
        results: &[I32],
    },
    HostFn {
        name: "now_ms",
        params: &[],
        results: &[I64],
    },
    HostFn {
        name: "monotonic_ms",
        params: &[],
        results: &[I64],
    },
    HostFn {
        name: "random",
        params: &[],
        results: &[I32],
    },
    HostFn {
        name: "log",
        params: &[I32, I32],
        results: &[I32],
    },
    HostFn {
        name: "exit",
        params: &[I32],
        results: &[],
    },
    HostFn {
        name: "clip_get",
        params: &[I32, I32],
        results: &[I32],
    },
    HostFn {
        name: "clip_set",
        params: &[I32, I32],
        results: &[I32],
    },
    HostFn {
        name: "fs_open",
        params: &[I32, I32, I32],
        results: &[I32],
    },
    HostFn {
        name: "fs_close",
        params: &[I32],
        results: &[I32],
    },
    HostFn {
        name: "fs_read",
        params: &[I32, I32, I32],
        results: &[I32],
    },
    HostFn {
        name: "fs_write",
        params: &[I32, I32, I32],
        results: &[I32],
    },
    HostFn {
        name: "fs_seek",
        params: &[I32, I64, I32],
        results: &[I64],
    },
    HostFn {
        name: "fs_stat",
        params: &[I32, I32, I32],
        results: &[I32],
    },
    HostFn {
        name: "fs_readdir",
        params: &[I32, I32, I32, I32, I32],
        results: &[I32],
    },
    HostFn {
        name: "fs_mkdir",
        params: &[I32, I32],
        results: &[I32],
    },
    HostFn {
        name: "fs_unlink",
        params: &[I32, I32],
        results: &[I32],
    },
    HostFn {
        name: "fs_rename",
        params: &[I32, I32, I32, I32],
        results: &[I32],
    },
    HostFn {
        name: "net_http_get",
        params: &[I32, I32, I32, I32],
        results: &[I32],
    },
];

/// The import module name.
pub const HOST_MODULE: &str = "osj";

/// The call id of `name` in [`HOST_FNS`].
pub fn host_fn_id(name: &str) -> Option<u32> {
    HOST_FNS
        .iter()
        .position(|f| f.name == name)
        .map(|i| i as u32)
}

/// The exports the host may call, indexed by [`Job::export`].
pub const EXPORT_NAMES: &[&str] = &[
    "on_resize",
    "on_key",
    "on_text",
    "on_pointer",
    "on_scroll",
    "on_tick",
    "on_close",
    "render",
];

// ---- system calls ----

/// `() -> len`: the size of the module the process must run, or a negative error.
pub const SYS_HOST_MODULE_LEN: u64 = 16;
/// `(ptr, cap) -> n`: copy the module bytes to `ptr` (at most `cap`); returns the bytes copied.
pub const SYS_HOST_MODULE_READ: u64 = 17;
/// `(ptr)`: block until a job is posted, then write a [`Job`] at `ptr`.
pub const SYS_HOST_JOB_WAIT: u64 = 18;
/// `(ptr) -> result`: a [`HostCall`] at `ptr`; blocks until the kernel has served it. The result
/// is the function's value (an `i32` is sign-extended), or [`CALL_FAULT`] / [`CALL_EXIT`].
pub const SYS_HOST_CALL: u64 = 19;
/// `(ptr)`: a [`Done`] at `ptr` ends the current job.
pub const SYS_HOST_JOB_DONE: u64 = 20;

/// The kernel refused the call as a fault of the guest (a pointer outside its memory...): the host
/// function must trap, the job then ends with [`DONE_FAULT`].
pub const CALL_FAULT: i64 = i64::MIN;
/// The guest called `exit`: the host function must stop the guest with that code
/// (`HostCall.args[0]`); the job ends with [`DONE_EXIT`].
pub const CALL_EXIT: i64 = i64::MIN + 1;

// ---- records ----

/// Job kinds.
pub const JOB_INIT: u32 = 1;
pub const JOB_CALL: u32 = 2;
pub const JOB_QUIT: u32 = 3;

/// A unit of work the kernel gives the host. 40 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Job {
    pub kind: u32,
    /// Index into [`EXPORT_NAMES`] (`JOB_CALL`).
    pub export: u32,
    /// Arguments of the export (as many as its signature takes, the rest zero).
    pub args: [i32; 4],
    /// Fuel for this job: one unit per WebAssembly instruction.
    pub fuel: u64,
    /// Cap on the guest's linear memory in bytes (`JOB_INIT`).
    pub mem_bytes: u64,
}

/// One imported function the guest called. 72 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostCall {
    /// Index into [`HOST_FNS`].
    pub id: u32,
    pub nargs: u32,
    /// The arguments, each value in the low bits (an `i32` sign-extended to 64).
    pub args: [u64; 6],
    /// Where the guest's linear memory is in the process, and how long it is right now.
    pub mem_base: u64,
    pub mem_len: u64,
}

/// How a job ended ([`Done::status`]).
pub const DONE_OK: u32 = 0;
/// The guest trapped (`msg` says why).
pub const DONE_TRAP: u32 = 1;
pub const DONE_OUT_OF_FUEL: u32 = 2;
/// The guest called `exit` (`code`).
pub const DONE_EXIT: u32 = 3;
/// A host call was refused as a fault of the guest (the kernel keeps the reason).
pub const DONE_FAULT: u32 = 4;
/// Instantiation or a missing/mismatched import failed (`msg` says why).
pub const DONE_LOAD_FAILED: u32 = 5;

/// The end of a job. 40 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Done {
    pub status: u32,
    pub code: i32,
    pub fuel_spent: u64,
    /// Size of the guest's linear memory in 64 KiB pages.
    pub mem_pages: u64,
    /// Optional text (at most [`MAX_MSG`] bytes, UTF-8) in the process's memory.
    pub msg_ptr: u64,
    pub msg_len: u64,
}

/// Longest message a [`Done`] carries.
pub const MAX_MSG: usize = 256;

macro_rules! bytes_of {
    ($t:ty, $n:expr, $($f:ident),*) => {
        impl $t {
            /// The record as the bytes that cross the boundary.
            pub fn to_bytes(&self) -> [u8; $n] {
                let mut b = [0u8; $n];
                let mut w = Writer { b: &mut b, at: 0 };
                $( w.put(&self.$f); )*
                b
            }
        }
    };
}

struct Writer<'a> {
    b: &'a mut [u8],
    at: usize,
}

trait Put {
    fn put_into(&self, w: &mut Writer<'_>);
}

macro_rules! put_int {
    ($($t:ty),*) => {$(
        impl Put for $t {
            fn put_into(&self, w: &mut Writer<'_>) {
                let b = self.to_le_bytes();
                w.b[w.at..w.at + b.len()].copy_from_slice(&b);
                w.at += b.len();
            }
        }
    )*};
}
put_int!(u32, i32, u64);

impl<T: Put, const N: usize> Put for [T; N] {
    fn put_into(&self, w: &mut Writer<'_>) {
        for x in self {
            x.put_into(w);
        }
    }
}

impl Writer<'_> {
    fn put<T: Put>(&mut self, v: &T) {
        v.put_into(self);
    }
}

bytes_of!(Job, 40, kind, export, args, fuel, mem_bytes);
bytes_of!(HostCall, 72, id, nargs, args, mem_base, mem_len);
bytes_of!(
    Done, 40, status, code, fuel_spent, mem_pages, msg_ptr, msg_len
);

/// Little-endian reader over a record's bytes.
struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn u32(&mut self) -> u32 {
        let v = u32::from_le_bytes([
            self.b[self.at],
            self.b[self.at + 1],
            self.b[self.at + 2],
            self.b[self.at + 3],
        ]);
        self.at += 4;
        v
    }
    fn u64(&mut self) -> u64 {
        let lo = u64::from(self.u32());
        lo | u64::from(self.u32()) << 32
    }
}

impl Job {
    pub fn from_bytes(b: &[u8; 40]) -> Job {
        let mut r = Reader { b, at: 0 };
        Job {
            kind: r.u32(),
            export: r.u32(),
            args: [
                r.u32() as i32,
                r.u32() as i32,
                r.u32() as i32,
                r.u32() as i32,
            ],
            fuel: r.u64(),
            mem_bytes: r.u64(),
        }
    }
}

impl HostCall {
    pub fn from_bytes(b: &[u8; 72]) -> HostCall {
        let mut r = Reader { b, at: 0 };
        HostCall {
            id: r.u32(),
            nargs: r.u32(),
            args: [r.u64(), r.u64(), r.u64(), r.u64(), r.u64(), r.u64()],
            mem_base: r.u64(),
            mem_len: r.u64(),
        }
    }
}

impl Done {
    pub fn from_bytes(b: &[u8; 40]) -> Done {
        let mut r = Reader { b, at: 0 };
        Done {
            status: r.u32(),
            code: r.u32() as i32,
            fuel_spent: r.u64(),
            mem_pages: r.u64(),
            msg_ptr: r.u64(),
            msg_len: r.u64(),
        }
    }
}
