//! The plain data the system interface exchanges: time, memory, disks, processes, users, network and programs.

use super::*;

/// Calendar date and wall-clock time (UTC or local, the kernel decides).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// Heap/physical memory in bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct MemInfo {
    pub total: u64,
    pub used: u64,
}

impl MemInfo {
    pub fn free(&self) -> u64 {
        self.total.saturating_sub(self.used)
    }
}

/// One mounted volume for `df`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DiskInfo {
    pub name: String,
    pub mount: String,
    pub total: u64,
    pub used: u64,
}

/// One process/thread for `ps`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProcInfo {
    pub pid: u32,
    pub name: String,
    /// Short state word such as `run`, `ready`, `sleep`.
    pub state: String,
    pub mem_bytes: u64,
}

/// Who is signed in, for `whoami` and `id`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Identity {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub gname: String,
    /// Every group the user is in (primary included), with names.
    pub groups: Vec<(u32, String)>,
    pub admin: bool,
}

/// One account for `users`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct UserEntry {
    pub name: String,
    pub uid: u32,
    pub full_name: String,
    pub admin: bool,
}

/// Outcome of a `ping`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PingStats {
    pub sent: u32,
    pub received: u32,
    /// Average round trip in microseconds (0 when nothing came back).
    pub avg_rtt_us: u64,
}

/// Why a system call failed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SysErr {
    /// The kernel does not provide this.
    Unsupported,
    /// No such pid.
    NoSuchProcess,
    /// The process may not be signalled.
    Denied,
    /// Name resolution or network failure.
    Network,
    /// The name does not exist (DNS answered, with no address).
    HostNotFound,
    /// Nothing answered in time.
    Timeout,
    /// The user pressed Ctrl+C while the call was waiting.
    Cancelled,
    /// The request was made but the transfer failed (TLS, redirect, size...).
    Failed,
}

impl SysErr {
    /// The catalog key of the text for this error.
    pub const fn key(self) -> &'static str {
        match self {
            SysErr::Unsupported => tk!("sh.sys.unsupported"),
            SysErr::NoSuchProcess => tk!("sh.sys.no_process"),
            SysErr::Denied => tk!("sh.sys.denied"),
            SysErr::Network => tk!("sh.sys.network"),
            SysErr::HostNotFound => tk!("sh.sys.host_not_found"),
            SysErr::Timeout => tk!("sh.sys.timeout"),
            SysErr::Cancelled => tk!("sh.sys.interrupted"),
            SysErr::Failed => tk!("sh.sys.failed"),
        }
    }

    /// The text of this error in the language in effect.
    pub fn message(self) -> &'static str {
        crate::i18n::tr(self.key())
    }
}

/// Why a ring 3 program could not start ([`SysInfo::spawn_program`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpawnFail {
    /// The system has no user processes.
    Unsupported,
    /// No embedded program has that name.
    Unknown,
    /// The file is not an ELF executable this system can run (the text says why in the log).
    BadImage,
    NoMemory,
    /// Too many processes already.
    TooMany,
    /// The arguments do not fit (4 KiB in all) or hold a NUL.
    BadArgs,
}

impl SpawnFail {
    /// The catalog key of the text for this failure.
    pub const fn key(self) -> &'static str {
        match self {
            SpawnFail::Unsupported => tk!("sh.sys.unsupported"),
            SpawnFail::Unknown => tk!("sh.run.unknown"),
            SpawnFail::BadImage => tk!("sh.run.bad_image"),
            SpawnFail::NoMemory => tk!("sh.run.no_memory"),
            SpawnFail::TooMany => tk!("sh.run.too_many"),
            SpawnFail::BadArgs => tk!("sh.run.bad_args"),
        }
    }

    pub fn message(self) -> &'static str {
        crate::i18n::tr(self.key())
    }
}

/// What a ring 3 program is started with besides its image: what `run` was given after the program's
/// name, the folder it runs in (relative paths in the program start from there) and what was piped to it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ProgramCtx {
    pub args: Vec<String>,
    pub cwd: String,
    pub stdin: Vec<u8>,
}

/// What a running ring 3 program has done since the last poll ([`SysInfo::poll_program`]).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ProgramPoll {
    /// Output written since the previous poll.
    pub output: Vec<u8>,
    /// `Some` once it is over.
    pub end: Option<ProgramEnd>,
}

/// How a ring 3 program ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProgramEnd {
    Exited(i32),
    /// A fault (bad memory access, privileged instruction...) or a kill.
    Killed,
}

/// A fetched HTTP(S) response ([`SysInfo::http_get`]).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct HttpResponse {
    /// Status code of the final response (after redirects), 0 if unknown.
    pub status: u16,
    /// Raw status line and headers of the final response (CRLF separated).
    pub head: Vec<u8>,
    /// The body, at most the `max_body` the caller asked for.
    pub body: Vec<u8>,
    /// The body was cut at `max_body` (or at the kernel's own cap).
    pub truncated: bool,
}

/// What `ifconfig` prints about the network interface.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct NetInfo {
    /// Driver / card name (`ne2000`, `virtio-net`).
    pub nic: String,
    pub link_up: bool,
    /// Dotted-quad address, `None` while no address is held.
    pub ip: Option<String>,
    /// Prefix length of the subnet (`24`).
    pub prefix: u8,
    pub gateway: Option<String>,
    pub dns: Vec<String>,
    /// `bound`, `static`, `init`... (free text).
    pub dhcp: String,
    pub rx_packets: u64,
    pub rx_bytes: u64,
    pub tx_packets: u64,
    pub tx_bytes: u64,
}
