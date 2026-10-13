//! TCP/IP networking via smoltcp, layered over a [`Port`] (whichever NIC driver
//! the boot picked). This is the browser's transport: DNS resolution, TCP
//! connections, and a blocking HTTP GET that drives the smoltcp poll loop until
//! the request completes. The `Net` *owns* the port, so nothing else can touch
//! the NIC while it exists unless it goes through [`Net::port`].
//!
//! The interface address, default route and DNS servers come from the
//! [`NetConfig`] that `netd` obtained over DHCP (or its static fallback, QEMU's
//! user-mode/SLIRP defaults 10.0.2.15/24, gateway 10.0.2.2, DNS 10.0.2.3) and
//! changes through [`Net::reconfigure`] when the lease is renewed with different
//! parameters or lost. The DHCP client is `kitsune_core::lease` run by `netd`;
//! smoltcp's own DHCP socket is deliberately not used, so there is a single owner
//! of the IP address.
//!
//! Names are resolved by `kitsune_core::dns` (not smoltcp's one-server DNS socket):
//! a TTL cache, every DHCP-supplied server, and failover with a timeout, over a
//! plain smoltcp UDP socket.

mod http;
mod tls;

use crate::interrupts;
use crate::netd::now_ms;
use crate::nic::{Port, STATS};
use crate::sync::RacyCell;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use embedded_tls::blocking::*;
use kitsune_core::browser::{Conn, FailReason, append_capped};
use smoltcp::iface::{Config, Interface, PollResult, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, IpAddress, IpCidr, IpEndpoint};

use kitsune_core::dns::{self, Cached, Outcome, Resolver, Step};
use kitsune_core::net::{DnsServers, Ipv4, NetConfig, parse_ipv4};

/// smoltcp `Instant` from the monotonic timer tick (TIMER_HZ).
fn now() -> Instant {
    let ms = interrupts::ticks() * 1000 / interrupts::TIMER_HZ as u64;
    Instant::from_millis(ms as i64)
}

/// A raw HTTP response (headers + body) as read from the network.
pub struct Fetched {
    pub data: Vec<u8>,
    /// The response was cut at [`MAX_RESPONSE_BYTES`]; the rest was discarded.
    pub truncated: bool,
    /// The server certificate of an https response (what the browser's popover shows).
    pub cert: Option<kitsune_core::browser::CertInfo>,
}

// ---- a Port as a smoltcp phy::Device ----

/// The device smoltcp drives: it owns the [`Port`].
pub struct Phy {
    port: Port,
}
pub struct Rx(Vec<u8>);
pub struct Tx<'a>(&'a mut Port);

impl Device for Phy {
    type RxToken<'a> = Rx;
    type TxToken<'a> = Tx<'a>;

    fn receive(&mut self, _t: Instant) -> Option<(Rx, Tx<'_>)> {
        let mut buf = [0u8; 1600];
        let len = self.port.poll(&mut buf)?;
        Some((Rx(buf[..len].to_vec()), Tx(&mut self.port)))
    }

    fn transmit(&mut self, _t: Instant) -> Option<Tx<'_>> {
        Some(Tx(&mut self.port))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut c = DeviceCapabilities::default();
        c.medium = Medium::Ethernet;
        c.max_transmission_unit = 1514;
        c
    }
}

impl RxToken for Rx {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

impl TxToken for Tx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut buf = vec![0u8; len];
        let r = f(&mut buf);
        self.0.send(&buf);
        r
    }
}

// ---- the stack ----

/// Empty polls in a row before a waiting worker thread sleeps (see `Net::pause`).
const QUIET_SPINS: u32 = 40;

pub struct Net {
    iface: Interface,
    sockets: SocketSet<'static>,
    device: Phy,
    /// Configuration in force; `None` once the lease is lost.
    cfg: Option<NetConfig>,
    tcp: SocketHandle,
    udp: SocketHandle,
    resolver: Resolver,
    /// An app is fetching: a name (or literal) that resolves to a local, private or
    /// reserved address is refused after resolution, whatever the app's URL said
    /// (`kitsune_core::appnet::ipv4_allowed`).
    public_only: bool,
    /// Polls in a row that found nothing to do (see [`Net::pause`]).
    quiet: u32,
}

/// Absolute tick by which the TLS handshake in progress must be done (0 = none).
/// The stream refuses to wait past it, so a server that dribbles bytes cannot
/// hold the fetcher for longer than `MAX_HANDSHAKE_MS`.
static HS_DEADLINE: AtomicU64 = AtomicU64::new(0);

impl Net {
    /// Build the stack for `cfg`: interface address with its prefix, a default
    /// route through the gateway (if any) and the DNS server (if any).
    pub fn new(port: Port, cfg: &NetConfig) -> Net {
        let mut device = Phy { port };
        let mut config = Config::new(EthernetAddress(device.port.mac().0).into());
        // smoltcp derives TCP initial sequence numbers and ephemeral choices from this seed; a
        // zero seed would make them guessable. Weak entropy here only costs predictability, and
        // the seed is replaced by the kernel generator's output, not a timestamp.
        config.random_seed = crate::rng::u64();
        let mut iface = Interface::new(config, &mut device, now());
        iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(IpCidr::new(IpAddress::Ipv4(cfg.ip.0.into()), cfg.prefix));
        });
        if let Some(gw) = cfg.gateway {
            let _ = iface.routes_mut().add_default_ipv4_route(gw.0.into());
        }

        let tcp_sock = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0u8; 8192]),
            tcp::SocketBuffer::new(vec![0u8; 8192]),
        );
        // UDP socket for DNS: a few datagrams in flight at most.
        let udp_sock = udp::Socket::new(
            udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 4], vec![0u8; 2048]),
            udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 4], vec![0u8; 1024]),
        );

        let mut sockets = SocketSet::new(vec![]);
        let tcp = sockets.add(tcp_sock);
        let udp = sockets.add(udp_sock);

        Net {
            iface,
            sockets,
            device,
            cfg: Some(*cfg),
            tcp,
            udp,
            resolver: Resolver::new(cfg.dns),
            public_only: false,
            quiet: 0,
        }
    }

    /// The NIC, for the frames smoltcp does not handle (DHCP, ARP/ping for the
    /// owner's own client). Only the network owner has a `Net`, and it uses this
    /// between fetches.
    pub fn port(&mut self) -> &mut Port {
        &mut self.device.port
    }

    /// The configuration in force (`None` after the lease was lost).
    pub fn config(&self) -> Option<&NetConfig> {
        self.cfg.as_ref()
    }

    /// Switch the interface to `cfg`: address, default route and resolver servers
    /// (`None`: no address, the lease is gone). Open connections are dropped and
    /// the DNS cache is flushed when the servers change.
    pub fn reconfigure(&mut self, cfg: Option<&NetConfig>) {
        self.sockets.get_mut::<tcp::Socket>(self.tcp).abort();
        self.iface.update_ip_addrs(|addrs| {
            addrs.clear();
            if let Some(c) = cfg {
                let _ = addrs.push(IpCidr::new(IpAddress::Ipv4(c.ip.0.into()), c.prefix));
            }
        });
        let routes = self.iface.routes_mut();
        routes.remove_default_ipv4_route();
        if let Some(gw) = cfg.and_then(|c| c.gateway) {
            let _ = routes.add_default_ipv4_route(gw.0.into());
        }
        self.resolver
            .set_servers(cfg.map_or(DnsServers::NONE, |c| c.dns));
        self.cfg = cfg.copied();
    }

    fn poll(&mut self) {
        let r = self.iface.poll(now(), &mut self.device, &mut self.sockets);
        self.quiet = if matches!(r, PollResult::None) {
            self.quiet.saturating_add(1)
        } else {
            0
        };
    }

    /// Drive the stack once and wait a little (see [`Net::pause`]).
    fn pump(&mut self) {
        self.poll();
        self.pause();
    }

    /// Wait between two polls of a stack with nothing to do yet. The NICs are polled (no
    /// interrupts), so a thread that just spun would keep a whole CPU at 100% for as long as the
    /// network takes to answer: a page download, a DNS or TLS round trip. Answers on a local link
    /// come back within a few polls, so spin that long; after that nothing is coming soon and the
    /// thread sleeps until the next timer tick (4 ms), letting the desktop and the idle `hlt` have
    /// the CPU. Only a worker thread with interrupts on can sleep (never the boot or compositor
    /// thread, which keep the old spin).
    pub(super) fn pause(&mut self) {
        if self.quiet < QUIET_SPINS
            || !x86_64::instructions::interrupts::are_enabled()
            || crate::sched::current() == 0
        {
            core::hint::spin_loop();
        } else {
            crate::sched::block(interrupts::ticks() + 1, || true);
        }
    }

    fn deadline(ms: u64) -> u64 {
        interrupts::ticks() + ms * interrupts::TIMER_HZ as u64 / 1000
    }

    /// Resolve `host` to an IPv4 address (bounded). A dotted-quad literal is its
    /// own answer and never goes to the network; otherwise the TTL cache is
    /// consulted, then the DHCP-supplied servers are asked in turn (rotating on
    /// timeout or SERVFAIL, see `kitsune_core::dns::Resolve`).
    pub(crate) fn resolve(&mut self, host: &str) -> Option<IpAddress> {
        if let Some(ip) = parse_ipv4(host.as_bytes()) {
            return Some(IpAddress::Ipv4(ip.0.into()));
        }
        STATS.on_dns_query();
        let start = now_ms();
        match self.resolver.lookup(host, start) {
            Some(Cached::Addr(a)) => {
                STATS.on_dns_cache_hit();
                crate::serial_println!("dns: {} -> {} (cache)", host, a);
                return Some(IpAddress::Ipv4(a.0.into()));
            }
            Some(Cached::NxDomain) => {
                STATS.on_dns_cache_hit();
                crate::serial_println!("dns: {} is an unknown name (cache)", host);
                return None;
            }
            None => {}
        }

        // An unpredictable id and source port are the only defense against a
        // forged answer from a host that cannot see our packets.
        let mut r = [0u8; 4];
        crate::rng::fill(&mut r);
        let id = u16::from_le_bytes([r[0], r[1]]);
        let local_port = 32768 + (u16::from_le_bytes([r[2], r[3]]) % 28000);
        let mut query = [0u8; 300];
        let qlen = dns::build_query(&mut query, id, host)?;
        {
            let s = self.sockets.get_mut::<udp::Socket>(self.udp);
            s.close();
            s.bind(local_port).ok()?;
        }

        let mut q = self.resolver.begin(host, id, start);
        let outcome = 'lookup: loop {
            match q.poll(now_ms()) {
                Step::Done(o) => break o,
                Step::Send { server, .. } => {
                    if q.attempts() > 1 {
                        STATS.on_dns_failover();
                        crate::serial_println!("dns: no answer for {}, trying {}", host, server);
                    }
                    let to = IpEndpoint::new(IpAddress::Ipv4(server.0.into()), dns::DNS_PORT);
                    let s = self.sockets.get_mut::<udp::Socket>(self.udp);
                    // smoltcp sends a socket's datagrams in order and a datagram whose next hop
                    // has not answered ARP stays at the head of the queue, blocking everything
                    // behind it: a dead first server would then starve the failover query. Closing
                    // the socket drops the stale datagram (a late answer to it is still accepted,
                    // it is matched by id and source, not by socket state).
                    s.close();
                    if s.bind(local_port).is_err() {
                        break Outcome::Failed;
                    }
                    let _ = s.send_slice(&query[..qlen], to);
                }
                Step::Wait(_) => {}
            }
            self.pump();
            let s = self.sockets.get_mut::<udp::Socket>(self.udp);
            while let Ok((data, meta)) = s.recv() {
                let IpAddress::Ipv4(from) = meta.endpoint.addr;
                if let Some(o) = q.on_response(Ipv4(from.octets()), data) {
                    break 'lookup o;
                }
            }
        };
        self.sockets.get_mut::<udp::Socket>(self.udp).close();
        self.resolver.finish(&q, outcome, now_ms());

        match outcome {
            Outcome::Resolved { addr, ttl } => {
                crate::serial_println!(
                    "dns: {} -> {} (ttl {} s, {} attempt{})",
                    host,
                    addr,
                    ttl,
                    q.attempts(),
                    if q.attempts() == 1 { "" } else { "s" }
                );
                Some(IpAddress::Ipv4(addr.0.into()))
            }
            Outcome::NxDomain | Outcome::NoData => {
                crate::serial_println!("dns: {} is an unknown name", host);
                None
            }
            Outcome::Failed | Outcome::NoServers => {
                STATS.on_dns_failure();
                crate::serial_println!("dns: {} failed (no answer from any server)", host);
                None
            }
        }
    }

    /// Tear the TCP connection down and let smoltcp put the RST on the wire (it
    /// is only queued by `abort`; without a poll the peer keeps a half-open
    /// connection and a single-threaded server never accepts the next one).
    fn abort_conn(&mut self) {
        self.sockets.get_mut::<tcp::Socket>(self.tcp).abort();
        for _ in 0..4 {
            self.poll();
        }
    }

    /// Restrict the next fetches to public destinations (apps), or lift the
    /// restriction (the browser). The name policy is `appnet::parse_url`; this repeats
    /// the check on the *resolved* address, which a name can point anywhere.
    pub fn set_public_only(&mut self, on: bool) {
        self.public_only = on;
    }

    /// Resolve for a fetch: [`FailReason::Dns`] when the name does not resolve.
    fn resolve_or_fail(&mut self, host: &str) -> Result<IpAddress, FailReason> {
        let ip = self.resolve(host).ok_or(FailReason::Dns)?;
        let IpAddress::Ipv4(a) = ip;
        if self.public_only && !kitsune_core::appnet::ipv4_allowed(a.octets()) {
            crate::serial_println!(
                "net: {} resolves to {}, a non-public address: refused",
                host,
                a
            );
            return Err(FailReason::Network);
        }
        Ok(ip)
    }

    /// One SNTP exchange with `server` (bounded to `timeout_ms`). Returns the
    /// validated measurement, or `None` (the reason is on the serial log).
    pub fn sntp_query(
        &mut self,
        server: IpAddress,
        timeout_ms: u64,
    ) -> Option<kitsune_core::sntp::Measurement> {
        use kitsune_core::sntp;
        let mut r = [0u8; 4];
        crate::rng::fill(&mut r);
        let local_port = 32768 + (u16::from_le_bytes([r[0], r[1]]) % 28000);
        let nonce = u16::from_le_bytes([r[2], r[3]]);
        {
            let s = self.sockets.get_mut::<udp::Socket>(self.udp);
            s.close();
            s.bind(local_port).ok()?;
        }
        let t1 = crate::clock::local_ntp(nonce);
        let req = sntp::build_request(t1);
        {
            let to = IpEndpoint::new(server, sntp::PORT);
            let s = self.sockets.get_mut::<udp::Socket>(self.udp);
            s.send_slice(&req, to).ok()?;
        }
        let end = Self::deadline(timeout_ms);
        let mut result = None;
        'wait: while interrupts::ticks() < end {
            self.pump();
            let s = self.sockets.get_mut::<udp::Socket>(self.udp);
            while let Ok((data, meta)) = s.recv() {
                if meta.endpoint.addr != server {
                    continue; // not from the server we asked
                }
                let t4 = crate::clock::local_ntp(0);
                match sntp::process_reply(t1, t4, data) {
                    Ok(m) => {
                        result = Some(m);
                        break 'wait;
                    }
                    Err(e) => {
                        crate::serial_println!(
                            "sntp: reply from {} rejected: {}",
                            server,
                            e.reason()
                        );
                    }
                }
            }
        }
        self.sockets.get_mut::<udp::Socket>(self.udp).close();
        result
    }

    /// Sync the trusted clock: try the time servers by name (DNS), then the
    /// gateway as a last resort. Logs every step; returns whether the clock is
    /// now confirmed.
    pub fn sync_time(&mut self) -> bool {
        const SERVERS: [&str; 3] = ["time.cloudflare.com", "pool.ntp.org", "time.google.com"];
        for name in SERVERS {
            let Some(ip) = self.resolve(name) else {
                continue;
            };
            for _ in 0..2 {
                if let Some(m) = self.sntp_query(ip, 1000) {
                    crate::serial_println!("sntp: {} answered", name);
                    crate::clock::apply_sntp(&m);
                    return true;
                }
            }
            crate::serial_println!("sntp: no valid answer from {}", name);
        }
        if let Some(gw) = self.cfg.and_then(|c| c.gateway) {
            let ip = IpAddress::Ipv4(gw.0.into());
            if let Some(m) = self.sntp_query(ip, 1500) {
                crate::serial_println!("sntp: gateway {} answered", gw);
                crate::clock::apply_sntp(&m);
                return true;
            }
        }
        crate::serial_println!("sntp: time NOT confirmed (using the RTC)");
        false
    }
}
