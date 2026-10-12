//! `SysInfo` for the shell: the snapshot of compositor facts and the thread-safe readers.

use super::jobs::CANCEL;
use crate::desktop::services::vfs;
use crate::desktop::*;
use alloc::string::ToString;
use core::sync::atomic::Ordering;
use kitsune_core::shell::sys::{
    DateTime, DiskInfo, HttpResponse, MemInfo, NetInfo, PingStats, ProcInfo, SysErr, SysInfo,
};
use kitsune_core::t;

/// What the compositor knows and the command thread needs, copied when a
/// command line is posted.
pub(crate) struct Snap {
    /// `ps`: the entries and whether `kill` may close them.
    pub procs: Vec<(ProcInfo, bool)>,
    pub mem: MemInfo,
    /// Identity of the two IDE disks (boot, filesystem).
    pub disks: [Option<crate::ata::DiskInfo>; 2],
}

/// `SysInfo` for one command line.
pub(crate) struct KSys {
    uid: u32,
    snap: Snap,
    /// `kill` requests for the compositor to carry out after the run.
    pub(super) kills: Vec<(u32, i32)>,
}

/// Scheduler ticks in `ms` milliseconds.
fn ticks_of(ms: u64) -> u64 {
    ms * u64::from(crate::interrupts::TIMER_HZ) / 1000
}

impl KSys {
    pub(super) fn new(uid: u32, snap: Snap) -> Self {
        Self {
            uid,
            snap,
            kills: Vec::new(),
        }
    }

    /// Wait `ms`, waking early on Ctrl+C. Returns `false` when interrupted.
    fn wait(&self, ms: u64) -> bool {
        let until = crate::interrupts::ticks() + ticks_of(ms);
        while crate::interrupts::ticks() < until {
            if self.interrupted() {
                return false;
            }
            crate::sched::block(until.min(crate::interrupts::ticks() + 5), || {
                !self.interrupted()
            });
        }
        true
    }

    /// An address from a dotted-quad literal or through DNS.
    fn address(&mut self, host: &str) -> Result<[u8; 4], SysErr> {
        if let Some(ip) = kitsune_core::shell::netcmds::parse_ipv4(host) {
            return Ok(ip);
        }
        let uid = self.uid;
        match crate::fetch::run_job(crate::fetch::NetJob::Resolve(host.to_string()), || {
            CANCEL.load(Ordering::Relaxed) == uid
        }) {
            Ok(crate::fetch::NetJobResult::Addr(Some(a))) => Ok(a),
            Ok(crate::fetch::NetJobResult::Addr(None)) => Err(SysErr::HostNotFound),
            Ok(_) => Err(SysErr::Failed),
            Err(e) => Err(job_err(e)),
        }
    }
}

fn job_err(e: crate::fetch::JobError) -> SysErr {
    use crate::fetch::JobError as J;
    match e {
        J::NoNetwork => SysErr::Network,
        J::Busy | J::Timeout => SysErr::Timeout,
        J::Cancelled => SysErr::Cancelled,
    }
}

fn fail_err(r: kitsune_core::browser::FailReason) -> SysErr {
    use kitsune_core::browser::FailReason as F;
    match r {
        F::Dns => SysErr::HostNotFound,
        F::Timeout => SysErr::Timeout,
        F::Network | F::Refused | F::WorkerDied => SysErr::Network,
        _ => SysErr::Failed,
    }
}

impl SysInfo for KSys {
    fn spawn_program(
        &mut self,
        name: &str,
        image: Option<&[u8]>,
        ctx: &kitsune_core::shell::ProgramCtx,
    ) -> Result<u32, kitsune_core::shell::SpawnFail> {
        use crate::userproc::{self, Launch, SpawnError};
        use kitsune_core::shell::SpawnFail;
        let bytes = match image {
            Some(b) => b,
            None => userproc::embedded(name).ok_or(SpawnFail::Unknown)?,
        };
        let args = kitsune_userabi::pack_args(&ctx.cwd, &ctx.args).ok_or(SpawnFail::BadArgs)?;
        let launch = Launch {
            parent: 0,
            cwd: ctx.cwd.clone(),
            args,
            stdin: ctx.stdin.clone(),
            // It acts as the user who runs it, even if the session changes meanwhile.
            cred: crate::desktop::services::accounts::cred(),
        };
        userproc::spawn_elf(name, bytes, launch).map_err(|e| match e {
            SpawnError::BadElf(why) => {
                crate::klog!(Warn, "run: '{name}' is not runnable: {why:?}");
                SpawnFail::BadImage
            }
            SpawnError::OutOfMemory => SpawnFail::NoMemory,
            SpawnError::TooMany => SpawnFail::TooMany,
            SpawnError::NoHost => SpawnFail::Unknown,
        })
    }

    fn poll_program(&mut self, pid: u32) -> kitsune_core::shell::ProgramPoll {
        use crate::userproc::End;
        use kitsune_core::shell::{ProgramEnd, ProgramPoll};
        let (output, end) = crate::userproc::poll(pid);
        ProgramPoll {
            output,
            end: end.map(|e| match e {
                End::Exited(c) => ProgramEnd::Exited(c),
                _ => ProgramEnd::Killed,
            }),
        }
    }

    fn kill_program(&mut self, pid: u32) {
        crate::userproc::kill(pid);
    }

    fn identity(&self) -> Option<kitsune_core::shell::sys::Identity> {
        crate::desktop::services::accounts::identity()
    }

    fn user_list(&self) -> Vec<kitsune_core::shell::sys::UserEntry> {
        crate::desktop::services::accounts::users()
            .into_iter()
            .map(|u| kitsune_core::shell::sys::UserEntry {
                name: u.name,
                uid: u.uid,
                full_name: u.full_name,
                admin: u.admin,
            })
            .collect()
    }

    fn lookup_user(&self, name: &str) -> Option<u32> {
        if name == "root" {
            return Some(0);
        }
        crate::desktop::services::accounts::lookup_user(name)
    }

    fn lookup_group(&self, name: &str) -> Option<u32> {
        crate::desktop::services::accounts::lookup_group(name)
    }

    fn user_name(&self, uid: u32) -> Option<String> {
        Some(crate::desktop::services::accounts::user_name(uid))
    }

    fn group_name(&self, gid: u32) -> Option<String> {
        Some(crate::desktop::services::accounts::group_name(gid))
    }

    fn now(&self) -> DateTime {
        // The SNTP-corrected clock when the network confirmed it, else the RTC value read at
        // boot and advanced by the timer; shifted to the zone the settings app chose. (The RTC
        // itself is not read here: its ports belong to the compositor thread.)
        let secs = crate::clock::trusted_unix_secs()
            .or_else(|| crate::clock::local_unix_ms().map(|ms| ms / 1000))
            .unwrap_or(0);
        let local = secs as i64 + i64::from(crate::rtc::tz_minutes()) * 60;
        let d = kitsune_core::hw::rtc::DateTime::from_epoch(local);
        DateTime {
            year: d.date.y,
            month: d.date.m,
            day: d.date.d,
            hour: d.time.h,
            minute: d.time.m,
            second: d.time.s,
        }
    }

    fn uptime_ms(&self) -> u64 {
        crate::netd::now_ms()
    }

    fn mem(&self) -> MemInfo {
        self.snap.mem
    }

    fn disks(&self) -> Vec<DiskInfo> {
        let u = vfs::statfs();
        let on_disk = vfs::volume() == vfs::Volume::Disk;
        let mut v = alloc::vec![DiskInfo {
            name: String::from(if on_disk { "ojfs3" } else { "ramfs" }),
            mount: String::from("/"),
            total: u.total,
            used: u.used(),
        }];
        // The raw drives: a drive that is in use as the volume above is not listed twice.
        for (i, d) in self.snap.disks.iter().enumerate() {
            let Some(d) = d else { continue };
            if i == 1 && on_disk {
                continue;
            }
            let total = d.sectors.saturating_mul(512);
            v.push(DiskInfo {
                name: alloc::format!("hd{}", (b'a' + i as u8) as char),
                mount: alloc::format!(
                    "({})",
                    if i == 0 {
                        t!("sh.df.boot_disk")
                    } else {
                        t!("sh.df.unused")
                    }
                ),
                total,
                used: if i == 0 { total } else { 0 },
            });
        }
        v
    }

    fn procs(&self) -> Vec<ProcInfo> {
        use crate::userproc::End;
        let mut v: Vec<ProcInfo> = self.snap.procs.iter().map(|(p, _)| p.clone()).collect();
        // Ring 3 processes follow the desktop's own table.
        v.extend(crate::userproc::list().into_iter().map(|p| ProcInfo {
            pid: p.pid,
            name: p.name,
            state: alloc::string::String::from(match p.end {
                End::Running => "ring3",
                End::Exited(_) => "done",
                End::Killed => "dead",
            }),
            mem_bytes: p.pages as u64 * 4096,
        }));
        v
    }

    fn kill(&mut self, pid: u32, signal: i32) -> Result<(), SysErr> {
        // A ring 3 process: any signal but 0 stops it.
        if crate::userproc::list().iter().any(|p| p.pid == pid) {
            if signal != 0 {
                crate::userproc::kill(pid);
            }
            return Ok(());
        }
        match self.snap.procs.iter().find(|(p, _)| p.pid == pid) {
            None => Err(SysErr::NoSuchProcess),
            Some((_, false)) => Err(SysErr::Denied),
            Some(_) => {
                // Signal 0 only asks whether the process exists.
                if signal != 0 {
                    self.kills.push((pid, signal));
                }
                Ok(())
            }
        }
    }

    fn ping(&mut self, host: &str, count: u32) -> Result<PingStats, SysErr> {
        use kitsune_core::icmp::PingError;
        let ip = self.address(host)?;
        let target = kitsune_core::net::Ipv4(ip);
        let mut stats = PingStats::default();
        let mut total_us = 0u64;
        let mut hard_fail = 0u32;
        for i in 0..count {
            if i > 0 && !self.wait(1000) {
                return Err(SysErr::Cancelled);
            }
            if self.interrupted() {
                return Err(SysErr::Cancelled);
            }
            stats.sent += 1;
            // A page load in progress keeps the NIC busy: wait for it (bounded).
            let mut tries = 0;
            let r = loop {
                match crate::netd::ping_us(target, 1000) {
                    Err(PingError::Busy) if tries < 100 && !self.interrupted() => {
                        tries += 1;
                        if !self.wait(100) {
                            return Err(SysErr::Cancelled);
                        }
                    }
                    other => break other,
                }
            };
            match r {
                Ok(us) => {
                    stats.received += 1;
                    total_us = total_us.saturating_add(us);
                }
                Err(PingError::NoNetwork | PingError::NoRoute | PingError::BadTarget) => {
                    hard_fail += 1;
                }
                // Timeout, unreachable, TTL exceeded, ARP: this echo got no reply.
                Err(_) => {}
            }
        }
        if stats.received == 0 && hard_fail == stats.sent {
            return Err(SysErr::Network);
        }
        stats.avg_rtt_us = total_us.checked_div(u64::from(stats.received)).unwrap_or(0);
        Ok(stats)
    }

    fn resolve(&mut self, host: &str) -> Result<Vec<[u8; 4]>, SysErr> {
        self.address(host).map(|a| alloc::vec![a])
    }

    fn http_get(&mut self, url: &str, max_body: usize) -> Result<HttpResponse, SysErr> {
        use crate::fetch::{NetJob, NetJobResult};
        let uid = self.uid;
        let r = crate::fetch::run_job(NetJob::Get(url.to_string()), || {
            CANCEL.load(Ordering::Relaxed) == uid
        })
        .map_err(job_err)?;
        let page = match r {
            NetJobResult::Page(p) => p,
            NetJobResult::Failed(f) => return Err(fail_err(f)),
            NetJobResult::Addr(_) => return Err(SysErr::Failed),
        };
        let data = page.data;
        let split = data
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map_or(data.len(), |i| i + 4);
        let decoded = kitsune_core::browser::page_body_partial(&data, page.truncated);
        let mut body = decoded.body;
        let mut truncated = page.truncated || decoded.note.is_some();
        if body.len() > max_body {
            body.truncate(max_body);
            truncated = true;
        }
        Ok(HttpResponse {
            status: kitsune_core::browser::status_code(&data).unwrap_or(0),
            head: data[..split].to_vec(),
            body,
            truncated,
        })
    }

    fn net_info(&self) -> Option<NetInfo> {
        let s = crate::netd::stats();
        let cfg = s.config;
        Some(NetInfo {
            nic: String::from(s.nic.name()),
            link_up: s.link_up,
            ip: cfg.map(|c| c.ip.to_string()),
            prefix: cfg.map_or(0, |c| c.prefix),
            gateway: cfg.and_then(|c| c.gateway).map(|g| g.to_string()),
            dns: cfg.map_or_else(Vec::new, |c| {
                c.dns.as_slice().iter().map(|d| d.to_string()).collect()
            }),
            dhcp: String::from(s.dhcp_state),
            rx_packets: s.rx_packets,
            rx_bytes: s.rx_bytes,
            tx_packets: s.tx_packets,
            tx_bytes: s.tx_bytes,
        })
    }

    fn interrupted(&self) -> bool {
        CANCEL.load(Ordering::Relaxed) == self.uid
    }

    fn sleep_ms(&mut self, ms: u64) {
        self.wait(ms);
    }

    fn hostname(&self) -> String {
        String::from("kitsune")
    }
}

// ---- commands that act on the desktop ----------------------------------------
