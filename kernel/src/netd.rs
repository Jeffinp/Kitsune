//! `netd`: the network owner. One value ([`Netd`]) holds the TCP/IP stack (and
//! through it the only handle on the NIC), the DHCP client and the ping client, and
//! it runs on the `fetcher` thread. There is no second party to race with: the
//! compositor never touches the hardware, it only posts requests through atomics
//! (`fetch::try_post` for pages, [`ping_start`] for echo requests) and reads
//! statistics ([`stats`]).
//!
//! # Who does what, and when
//!
//! The fetcher thread alternates between two jobs:
//!
//! * **A fetch** (it was woken by `fetch::try_post`): it drives smoltcp until the
//!   page is done. The NIC belongs to smoltcp for that time; incoming ARP and echo
//!   requests are answered by smoltcp itself, and the DHCP timers simply wait (a
//!   fetch is bounded: the longest is tens of seconds, the shortest lease timer is
//!   T1 = lease / 2).
//! * **Service** ([`Netd::service`], every [`IDLE_POLL_TICKS`] ticks of 4 ms while
//!   idle, every tick while a ping is running): drain the receive ring, then
//!   dispatch each frame (DHCP replies go to the lease machine, ARP replies and ICMP
//!   to the ping client, ARP/echo requests to the responder), fire the lease timers
//!   (T1 RENEW, T2 REBIND, expiry) and advance the ping.
//!
//! Boot is the same code run to completion before the thread exists
//! ([`Netd::boot`]): the first DISCOVER/REQUEST exchange (bounded by
//! [`BOOT_BUDGET_MS`]), then the static fallback if nothing answered. The lease
//! machine keeps trying in the background after a fallback, so a DHCP server that
//! appears later still configures the interface.
//!
//! # Lease changes
//!
//! `Bound` and `Reconfigured` replace the interface address, route and DNS servers
//! (`Net::reconfigure`) and announce the address with a gratuitous ARP. `Renewed`
//! only moves the lease clock. `Lost` (expiry or NAK) removes the address: the stack
//! then refuses connections instead of using an address that is no longer ours.
//!
//! # Timing
//!
//! There is no NIC interrupt: a received frame waits for the next service wake
//! (at most one 4 ms tick while a ping runs, [`IDLE_POLL_TICKS`] otherwise), so a
//! measured round trip includes up to one tick of that latency.

use crate::netstack::Net;
use crate::nic::{Port, STATS};
use crate::{interrupts, io, serial_println};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use kitsune_core::icmp::{self, Echo, Ping, PingError};
use kitsune_core::lease::{Action, Lease, Request};
use kitsune_core::net::{self, DHCP_BUILD_MIN, DhcpReply, Ipv4, Mac, NetConfig};

mod mailbox;
pub use mailbox::*;

mod pinger;

/// While idle the service runs this often (ticks of 4 ms).
pub const IDLE_POLL_TICKS: u64 = 4;
/// Frames dispatched per service wake: a flood must not starve the rest of the system.
const RX_BUDGET: usize = 32;
/// Longest the boot waits for a DHCP lease before using the static fallback.
const BOOT_BUDGET_MS: u64 = 2000;
/// ARP resolution for a ping: retries and the wait for each.
const ARP_TRIES: u32 = 3;
const ARP_WAIT_MS: u64 = 300;
/// Next hops remembered for ping.
const ARP_SLOTS: usize = 4;
/// How long a learned next-hop MAC is trusted.
const ARP_TTL_MS: u64 = 60_000;
/// Echo payload size (the classic 56 bytes).
const PING_PAYLOAD: usize = 56;

// ---- clocks ----
static TSC_KHZ: AtomicU64 = AtomicU64::new(0);
/// Tell netd the calibrated TSC rate (for microsecond timestamps).
pub fn set_tsc_khz(khz: u64) {
    TSC_KHZ.store(khz, Ordering::Relaxed);
}
/// The calibrated TSC rate in kHz (0 before the calibration).
pub fn tsc_khz() -> u64 {
    TSC_KHZ.load(Ordering::Relaxed)
}
/// Monotonic milliseconds (timer ticks).
pub fn now_ms() -> u64 {
    interrupts::ticks() * 1000 / u64::from(interrupts::TIMER_HZ)
}
/// Monotonic microseconds: the TSC when calibrated, else the timer tick.
pub fn now_us() -> u64 {
    let khz = TSC_KHZ.load(Ordering::Relaxed);
    if khz == 0 {
        interrupts::ticks() * 1_000_000 / u64::from(interrupts::TIMER_HZ)
    } else {
        (u128::from(io::rdtsc()) * 1000 / u128::from(khz)) as u64
    }
}

// ---- the owner ----

/// What the ping client is doing.
enum PingJob {
    Idle,
    /// Resolving the next hop's MAC.
    Arping {
        target: Ipv4,
        hop: Ipv4,
        timeout_ms: u32,
        tries: u32,
        next_try_ms: u64,
    },
    /// Echo sent, waiting for the reply.
    Waiting(Ping),
}
/// A change of interface configuration decided by the lease machine.
enum Change {
    None,
    Bound(NetConfig),
    Renewed(NetConfig),
    Reconfigured(NetConfig),
    Lost,
}
pub struct Netd {
    net: Net,
    lease: Lease,
    ping: PingJob,
    /// Next hops learned for ping, `(ip, mac, learned_ms)`; the oldest is replaced.
    arp: [Option<(Ipv4, Mac, u64)>; ARP_SLOTS],
    ping_seq: u16,
    ping_id: u16,
}
const _: () = assert!(DHCP_BUILD_MIN <= 600);
/// Send the frame a lease [`Action`] asks for; report the configuration change it
/// carries, if any.
fn execute(port: &mut Port, mac: Mac, action: Action) -> Change {
    let mut buf = [0u8; 600];
    match action {
        Action::SendDiscover { xid } => {
            let n = net::dhcp_discover(&mut buf, mac, xid);
            port.send(&buf[..n]);
            Change::None
        }
        Action::SendRequest { xid, kind } => {
            let n = match kind {
                Request::Select { ip, server } => net::dhcp_request(&mut buf, mac, xid, ip, server),
                Request::Renew {
                    ip,
                    server,
                    server_mac,
                } => {
                    STATS.on_dhcp_renewal();
                    serial_println!("net: DHCP RENEW {} unicast to {} (T1)", ip, server);
                    net::dhcp_request_renew(&mut buf, mac, xid, ip, Some((server_mac, server)))
                }
                Request::Rebind { ip } => {
                    STATS.on_dhcp_rebind();
                    serial_println!("net: DHCP REBIND {} broadcast (T2)", ip);
                    net::dhcp_request_renew(&mut buf, mac, xid, ip, None)
                }
            };
            port.send(&buf[..n]);
            Change::None
        }
        Action::SendRelease {
            xid,
            ip,
            server,
            server_mac,
        } => {
            let n = net::dhcp_release(&mut buf, mac, xid, ip, server_mac, server);
            port.send(&buf[..n]);
            Change::None
        }
        Action::Bound(c) => Change::Bound(c),
        Action::Renewed(c) => Change::Renewed(c),
        Action::Reconfigured(c) => Change::Reconfigured(c),
        Action::Lost => Change::Lost,
    }
}
fn log_lease(prefix: &str, cfg: &NetConfig) {
    serial_println!("net: {} {}", prefix, cfg);
    match cfg.lease_secs {
        Some(s) => serial_println!("net: lease time {} s", s),
        None => serial_println!("net: lease time infinite"),
    }
}
impl Netd {
    /// Bring the interface up: run the DHCP exchange (bounded by
    /// [`BOOT_BUDGET_MS`]), fall back to the static configuration if nobody answers,
    /// announce the address with a gratuitous ARP and build the stack around `port`.
    /// The DHCP client keeps running afterwards (see the module docs).
    pub fn boot(mut port: Port) -> Netd {
        let mac = port.mac();
        let seed = crate::rng::u32();
        let mut lease = Lease::new(mac, seed);
        let t0 = now_ms();
        lease.start(t0);
        STATS.set_dhcp_state(lease.state());

        let mut rx = [0u8; 1600];
        let mut leased = None;
        'wait: while leased.is_none() && now_ms() < t0 + BOOT_BUDGET_MS {
            let now = now_ms();
            for _ in 0..4 {
                let Some(a) = lease.poll(now) else { break };
                let _ = execute(&mut port, mac, a);
            }
            while let Some(n) = port.poll(&mut rx) {
                if let Some(r) = net::parse_dhcp(&rx[..n], mac)
                    && let Some(a) = lease.on_reply(now, &r)
                    && let Change::Bound(c) = execute(&mut port, mac, a)
                {
                    leased = Some(c);
                    break 'wait;
                }
            }
            core::hint::spin_loop();
        }
        STATS.set_dhcp_state(lease.state());

        let cfg = match leased {
            Some(c) => {
                log_lease("DHCP lease", &c);
                c
            }
            None => {
                let why = match lease.state() {
                    kitsune_core::lease::State::Requesting => "no DHCP ack",
                    _ => "no DHCP offer",
                };
                serial_println!("net: static fallback ({})", why);
                NetConfig::STATIC_FALLBACK
            }
        };
        STATS.set_config(Some(&cfg), now_ms());

        let mut frame = [0u8; 64];
        let len = net::arp_announce(&mut frame, mac, cfg.ip);
        port.send(&frame[..len]);

        let n = Netd {
            net: Net::new(port, &cfg),
            lease,
            ping: PingJob::Idle,
            arp: [None; ARP_SLOTS],
            ping_seq: 0,
            ping_id: (seed >> 8) as u16,
        };
        UP.store(true, Ordering::Release);
        n
    }

    /// The TCP/IP stack, for a fetch.
    pub fn net_mut(&mut self) -> &mut Net {
        &mut self.net
    }

    fn apply(&mut self, change: Change) {
        let now = now_ms();
        match change {
            Change::None => {}
            Change::Bound(cfg) | Change::Reconfigured(cfg) => {
                self.net.reconfigure(Some(&cfg));
                STATS.set_config(Some(&cfg), now);
                self.arp = [None; ARP_SLOTS];
                log_lease("DHCP lease", &cfg);
                let mac = self.net.port().mac();
                let mut frame = [0u8; 64];
                let len = net::arp_announce(&mut frame, mac, cfg.ip);
                self.net.port().send(&frame[..len]);
            }
            Change::Renewed(cfg) => {
                STATS.set_config(Some(&cfg), now);
                match cfg.lease_secs {
                    Some(s) => serial_println!("net: DHCP lease renewed for {} s", s),
                    None => serial_println!("net: DHCP lease renewed (infinite)"),
                }
            }
            Change::Lost => {
                self.net.reconfigure(None);
                STATS.set_config(None, now);
                STATS.on_dhcp_lost();
                self.arp = [None; ARP_SLOTS];
                serial_println!("net: DHCP lease expired; address dropped, searching again");
            }
        }
        STATS.set_dhcp_state(self.lease.state());
    }

    /// Feed one DHCP reply to the lease machine.
    fn lease_reply(&mut self, now: u64, r: &DhcpReply) {
        if let Some(a) = self.lease.on_reply(now, r) {
            let mac = self.net.port().mac();
            let change = execute(self.net.port(), mac, a);
            self.apply(change);
        }
        STATS.set_dhcp_state(self.lease.state());
    }

    /// One service pass; returns how many ticks the owner may sleep before the next.
    pub fn service(&mut self) -> u64 {
        let now = now_ms();
        let mac = self.net.port().mac();
        STATS.set_link(self.net.port().link_up());

        // 1. Receive: dispatch what is waiting.
        let mut rx = [0u8; 1600];
        let mut tx = [0u8; 1600];
        for _ in 0..RX_BUDGET {
            let Some(len) = self.net.port().poll(&mut rx) else {
                break;
            };
            let frame = &rx[..len];
            if let Some(r) = net::parse_dhcp(frame, mac) {
                self.lease_reply(now, &r);
                continue;
            }
            let our_ip = self.net.config().map(|c| c.ip);
            if let Some(ip) = our_ip {
                self.on_ping_frame(frame, ip);
                if let Some(n) = net::respond(frame, mac, ip, &mut tx) {
                    self.net.port().send(&tx[..n]);
                }
            }
        }

        // 2. DHCP timers.
        for _ in 0..4 {
            let Some(a) = self.lease.poll(now) else { break };
            let change = execute(self.net.port(), mac, a);
            self.apply(change);
        }
        STATS.set_dhcp_state(self.lease.state());

        // 3. Ping.
        self.ping_step(now);

        if matches!(self.ping, PingJob::Idle) {
            IDLE_POLL_TICKS
        } else {
            1
        }
    }
}
