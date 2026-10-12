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

mod mock;
mod types;
pub use mock::*;
pub use types::*;

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
    /// `name` when there are none, with the arguments, folder and input in `ctx`. Returns its process id.
    fn spawn_program(
        &mut self,
        _name: &str,
        _image: Option<&[u8]>,
        _ctx: &ProgramCtx,
    ) -> Result<u32, SpawnFail> {
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
#[cfg(test)]
mod tests;
