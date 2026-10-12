//! Two implementations of [`SysInfo`]: a scripted [`MockSys`] for tests and a [`MinimalSys`] with only the required methods.

use super::*;

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
    /// What each started program was given, in the same order as `started`.
    pub started_with: Vec<ProgramCtx>,
    pub program_kills: Vec<u32>,
    /// `interrupted` turns true after this many polls (`None` = never).
    pub interrupt_after: Option<u32>,
    pub(super) polls: core::cell::Cell<u32>,
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
            started_with: Vec::new(),
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

    fn spawn_program(
        &mut self,
        name: &str,
        image: Option<&[u8]>,
        ctx: &ProgramCtx,
    ) -> Result<u32, SpawnFail> {
        self.started_with.push(ctx.clone());
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
