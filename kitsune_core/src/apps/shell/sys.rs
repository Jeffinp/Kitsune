//! Everything the shell needs from the running system, as one trait.
//!
//! The kernel implements [`SysInfo`] once (RTC, PIT ticks, heap statistics,
//! scheduler, network stack) and hands it to the shell with the filesystem.
//! Only [`SysInfo::now`], [`SysInfo::uptime_ms`] and [`SysInfo::mem`] are
//! required; the others default to "not supported" / no-ops so a partial kernel
//! implementation still works and the command prints a clear message.
//!
//! All numbers are integers (the kernel is soft-float).

use crate::tk;
use alloc::string::String;
use alloc::vec::Vec;

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
        }
    }

    pub fn message(self) -> &'static str {
        crate::i18n::tr(self.key())
    }
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

/// System services behind `date`, `uptime`, `free`, `df`, `ps`, `kill`,
/// `ping`, `sleep` and `clear`.
pub trait SysInfo {
    /// Current date and time (`date`).
    fn now(&self) -> DateTime;

    /// Milliseconds since boot (`uptime`).
    fn uptime_ms(&self) -> u64;

    /// Memory totals (`free`).
    fn mem(&self) -> MemInfo;

    /// Mounted volumes (`df`). The default is empty; `df` then reports the
    /// [`crate::apps::shell::ShellFs::usage`] of the root filesystem.
    fn disks(&self) -> Vec<DiskInfo> {
        Vec::new()
    }

    /// Running processes (`ps`).
    fn procs(&self) -> Vec<ProcInfo> {
        Vec::new()
    }

    /// Send `signal` (9 = kill, 15 = terminate) to `pid` (`kill`).
    fn kill(&mut self, _pid: u32, _signal: i32) -> Result<(), SysErr> {
        Err(SysErr::Unsupported)
    }

    /// Send `count` echo requests to `host` (`ping`); blocks until done.
    fn ping(&mut self, _host: &str, _count: u32) -> Result<PingStats, SysErr> {
        Err(SysErr::Unsupported)
    }

    /// Resolve `host` to its IPv4 addresses through DNS (`nslookup`); blocks
    /// until the answer or a timeout. An address literal need not be resolved
    /// by the implementation (the builtin handles it).
    fn resolve(&mut self, _host: &str) -> Result<Vec<[u8; 4]>, SysErr> {
        Err(SysErr::Unsupported)
    }

    /// Fetch `url` (`http://` or `https://`), following redirects, keeping at
    /// most `max_body` bytes of the body (`curl`, `wget`); blocks until done.
    fn http_get(&mut self, _url: &str, _max_body: usize) -> Result<HttpResponse, SysErr> {
        Err(SysErr::Unsupported)
    }

    /// The network interface state (`ifconfig`); `None` without a NIC.
    fn net_info(&self) -> Option<NetInfo> {
        None
    }

    /// True once the user asked to stop the running command (Ctrl+C). The
    /// executor polls it between commands and loop iterations and aborts with
    /// status 130; implementations of long waits (`sleep_ms`, `ping`,
    /// `http_get`) should poll it too and return early.
    fn interrupted(&self) -> bool {
        false
    }

    /// The signed-in user (`whoami`, `id`, `groups`); `None` when the system has no accounts.
    fn identity(&self) -> Option<Identity> {
        None
    }

    /// Every account (`users`).
    fn user_list(&self) -> Vec<UserEntry> {
        Vec::new()
    }

    /// The uid of user `name`, for `chown`.
    fn lookup_user(&self, name: &str) -> Option<u32> {
        let _ = name;
        None
    }

    /// The gid of group `name`, for `chown` and `chgrp`.
    fn lookup_group(&self, name: &str) -> Option<u32> {
        let _ = name;
        None
    }

    /// The name of uid `uid`, for `ls -l`.
    fn user_name(&self, uid: u32) -> Option<String> {
        let _ = uid;
        None
    }

    /// The name of gid `gid`, for `ls -l`.
    fn group_name(&self, gid: u32) -> Option<String> {
        let _ = gid;
        None
    }

    /// Start a ring 3 program (`run`): the ELF bytes in `image`, or the embedded program called
    /// `name` when there are none. Returns its process id.
    fn spawn_program(&mut self, _name: &str, _image: Option<&[u8]>) -> Result<u32, SpawnFail> {
        Err(SpawnFail::Unsupported)
    }

    /// What the program `pid` wrote since the last call and whether it is over.
    fn poll_program(&mut self, _pid: u32) -> ProgramPoll {
        ProgramPoll {
            output: Vec::new(),
            end: Some(ProgramEnd::Killed),
        }
    }

    /// Stop the program `pid` (Ctrl+C while `run` waits).
    fn kill_program(&mut self, _pid: u32) {}

    /// Wait `ms` milliseconds (`sleep`). The shell already caps the value with
    /// [`crate::apps::shell::Limits::max_sleep_ms`].
    fn sleep_ms(&mut self, _ms: u64) {}

    /// Clear the terminal (`clear`).
    fn clear_screen(&mut self) {}

    /// Host name for the prompt.
    fn hostname(&self) -> String {
        String::from("kitsune")
    }
}

/// A deterministic [`SysInfo`] for tests and fuzzing.
#[derive(Clone, Debug)]
pub struct MockSys {
    pub time: DateTime,
    pub uptime: u64,
    pub memory: MemInfo,
    pub disk_list: Vec<DiskInfo>,
    pub process_list: Vec<ProcInfo>,
    /// Every `sleep_ms` call, in order.
    pub slept: Vec<u64>,
    pub cleared: u32,
    pub killed: Vec<(u32, i32)>,
    pub ping_ok: bool,
    /// Name -> address answered by `resolve` (several entries = several addresses).
    pub dns: Vec<(String, [u8; 4])>,
    /// URL -> response served by `http_get`; the URLs asked for go to `fetched`.
    pub web: Vec<(String, HttpResponse)>,
    pub fetched: Vec<String>,
    pub net: Option<NetInfo>,
    /// Who `whoami`/`id` report (`None` = no accounts).
    pub who: Option<Identity>,
    /// Accounts for `users`, `chown` and `ls -l` (name, uid); groups are (name, gid).
    pub people: Vec<UserEntry>,
    pub group_list: Vec<(String, u32)>,
    /// Programs `run` knows (name, output, exit status; `None` = killed by a fault).
    pub programs: Vec<(String, Vec<u8>, Option<i32>)>,
    /// Programs started through `spawn_program` (name), and the pids killed.
    pub started: Vec<String>,
    pub program_kills: Vec<u32>,
    /// `interrupted` turns true after this many polls (`None` = never).
    pub interrupt_after: Option<u32>,
    polls: core::cell::Cell<u32>,
}

impl Default for MockSys {
    fn default() -> Self {
        Self {
            time: DateTime {
                year: 2026,
                month: 10,
                day: 7,
                hour: 13,
                minute: 5,
                second: 9,
            },
            uptime: 93_784_000,
            memory: MemInfo {
                total: 64 * 1024 * 1024,
                used: 20 * 1024 * 1024,
            },
            disk_list: Vec::new(),
            process_list: alloc::vec![
                ProcInfo {
                    pid: 1,
                    name: String::from("init"),
                    state: String::from("run"),
                    mem_bytes: 4096,
                },
                ProcInfo {
                    pid: 7,
                    name: String::from("shell"),
                    state: String::from("ready"),
                    mem_bytes: 8192,
                },
            ],
            slept: Vec::new(),
            cleared: 0,
            killed: Vec::new(),
            ping_ok: true,
            dns: Vec::new(),
            web: Vec::new(),
            fetched: Vec::new(),
            net: None,
            who: None,
            people: Vec::new(),
            group_list: Vec::new(),
            programs: Vec::new(),
            started: Vec::new(),
            program_kills: Vec::new(),
            interrupt_after: None,
            polls: core::cell::Cell::new(0),
        }
    }
}

impl SysInfo for MockSys {
    fn now(&self) -> DateTime {
        self.time
    }

    fn uptime_ms(&self) -> u64 {
        self.uptime
    }

    fn mem(&self) -> MemInfo {
        self.memory
    }

    fn disks(&self) -> Vec<DiskInfo> {
        self.disk_list.clone()
    }

    fn procs(&self) -> Vec<ProcInfo> {
        self.process_list.clone()
    }

    fn kill(&mut self, pid: u32, signal: i32) -> Result<(), SysErr> {
        if self.process_list.iter().any(|p| p.pid == pid) {
            self.killed.push((pid, signal));
            self.process_list.retain(|p| p.pid != pid);
            Ok(())
        } else {
            Err(SysErr::NoSuchProcess)
        }
    }

    fn ping(&mut self, _host: &str, count: u32) -> Result<PingStats, SysErr> {
        if self.ping_ok {
            Ok(PingStats {
                sent: count,
                received: count,
                avg_rtt_us: 1500,
            })
        } else {
            Err(SysErr::Network)
        }
    }

    fn resolve(&mut self, host: &str) -> Result<Vec<[u8; 4]>, SysErr> {
        let found: Vec<[u8; 4]> = self
            .dns
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(host))
            .map(|(_, a)| *a)
            .collect();
        if found.is_empty() {
            Err(SysErr::HostNotFound)
        } else {
            Ok(found)
        }
    }

    fn http_get(&mut self, url: &str, max_body: usize) -> Result<HttpResponse, SysErr> {
        self.fetched.push(String::from(url));
        let Some((_, r)) = self.web.iter().find(|(u, _)| u == url) else {
            return Err(SysErr::Network);
        };
        let mut r = r.clone();
        if r.body.len() > max_body {
            r.body.truncate(max_body);
            r.truncated = true;
        }
        Ok(r)
    }

    fn net_info(&self) -> Option<NetInfo> {
        self.net.clone()
    }

    fn identity(&self) -> Option<Identity> {
        self.who.clone()
    }

    fn spawn_program(&mut self, name: &str, image: Option<&[u8]>) -> Result<u32, SpawnFail> {
        if let Some(img) = image {
            if !img.starts_with(b"\x7fELF") {
                return Err(SpawnFail::BadImage);
            }
            self.started.push(String::from(name));
            return Ok(5000 + self.started.len() as u32);
        }
        if !self.programs.iter().any(|(n, ..)| n == name) {
            return Err(SpawnFail::Unknown);
        }
        self.started.push(String::from(name));
        Ok(5000 + self.started.len() as u32)
    }

    fn poll_program(&mut self, pid: u32) -> ProgramPoll {
        let name = self
            .started
            .get(pid as usize - 5001)
            .cloned()
            .unwrap_or_default();
        match self.programs.iter().find(|(n, ..)| *n == name) {
            Some((_, out, code)) => ProgramPoll {
                output: out.clone(),
                end: Some(code.map_or(ProgramEnd::Killed, ProgramEnd::Exited)),
            },
            None => ProgramPoll {
                output: Vec::new(),
                end: Some(ProgramEnd::Exited(0)),
            },
        }
    }

    fn kill_program(&mut self, pid: u32) {
        self.program_kills.push(pid);
    }

    fn user_list(&self) -> Vec<UserEntry> {
        self.people.clone()
    }

    fn lookup_user(&self, name: &str) -> Option<u32> {
        self.people.iter().find(|u| u.name == name).map(|u| u.uid)
    }

    fn lookup_group(&self, name: &str) -> Option<u32> {
        self.group_list
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, g)| *g)
    }

    fn user_name(&self, uid: u32) -> Option<String> {
        if uid == 0 {
            return Some(String::from("root"));
        }
        self.people
            .iter()
            .find(|u| u.uid == uid)
            .map(|u| u.name.clone())
    }

    fn group_name(&self, gid: u32) -> Option<String> {
        self.group_list
            .iter()
            .find(|(_, g)| *g == gid)
            .map(|(n, _)| n.clone())
    }

    fn interrupted(&self) -> bool {
        let n = self.polls.get().saturating_add(1);
        self.polls.set(n);
        self.interrupt_after.is_some_and(|k| n > k)
    }

    fn sleep_ms(&mut self, ms: u64) {
        self.slept.push(ms);
        self.uptime += ms;
    }

    fn clear_screen(&mut self) {
        self.cleared += 1;
    }
}

/// A [`SysInfo`] with only the required methods (everything else defaults),
/// for tests of the "not supported" paths.
#[derive(Clone, Copy, Debug, Default)]
pub struct MinimalSys;

impl SysInfo for MinimalSys {
    fn now(&self) -> DateTime {
        DateTime::default()
    }

    fn uptime_ms(&self) -> u64 {
        0
    }

    fn mem(&self) -> MemInfo {
        MemInfo::default()
    }
}

#[cfg(test)]
mod tests;
