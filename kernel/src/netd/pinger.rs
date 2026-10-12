//! The ping client of `Netd`: echo requests, the ARP cache of the next hop and the ping state machine.

use super::*;

impl Netd {
    pub(super) fn finish_ping(&mut self, r: Result<u64, PingError>) {
        self.ping = PingJob::Idle;
        if r.is_ok() {
            STATS.on_ping_ok();
        }
        P_RESULT.store(encode_result(r), Ordering::Release);
        P_STATE.store(P_DONE, Ordering::Release);
    }

    pub(super) fn on_ping_frame(&mut self, frame: &[u8], our_ip: Ipv4) {
        if matches!(self.ping, PingJob::Idle) {
            return;
        }
        let now = now_ms();
        if let Some((ip, mac)) = net::parse_arp_reply(frame, our_ip) {
            self.arp_learn(ip, mac, now);
            return;
        }
        if let PingJob::Waiting(p) = &mut self.ping
            && let Some(ev) = icmp::parse_event(frame, our_ip)
            && let Some(r) = p.on_event(now_us(), &ev)
        {
            self.finish_ping(r);
        }
    }

    /// Remember `ip -> mac`: refresh an existing entry, else take a free slot, else
    /// replace the oldest.
    pub(super) fn arp_learn(&mut self, ip: Ipv4, mac: Mac, now: u64) {
        let slot = self
            .arp
            .iter()
            .position(|e| e.is_some_and(|(i, _, _)| i == ip))
            .or_else(|| self.arp.iter().position(Option::is_none))
            .unwrap_or_else(|| {
                (0..ARP_SLOTS)
                    .min_by_key(|&i| self.arp[i].map_or(0, |(_, _, t)| t))
                    .unwrap_or(0)
            });
        self.arp[slot] = Some((ip, mac, now));
    }

    /// Cached next-hop MAC, if still fresh.
    pub(super) fn arp_lookup(&self, hop: Ipv4, now: u64) -> Option<Mac> {
        self.arp
            .iter()
            .flatten()
            .find(|&&(ip, _, t)| ip == hop && now.saturating_sub(t) < ARP_TTL_MS)
            .map(|&(_, m, _)| m)
    }

    pub(super) fn send_echo(
        &mut self,
        cfg: &NetConfig,
        target: Ipv4,
        hop_mac: Mac,
        timeout_ms: u32,
    ) {
        self.ping_seq = self.ping_seq.wrapping_add(1);
        let mac = self.net.port().mac();
        let mut buf = [0u8; icmp::frame_len(PING_PAYLOAD)];
        let e = Echo {
            src_mac: mac,
            dst_mac: hop_mac,
            src_ip: cfg.ip,
            dst_ip: target,
            id: self.ping_id,
            seq: self.ping_seq,
            payload: PING_PAYLOAD,
        };
        let Some(n) = icmp::build_echo_request(&mut buf, &e) else {
            self.finish_ping(Err(PingError::BadTarget));
            return;
        };
        let sent = now_us();
        if !self.net.port().send(&buf[..n]) {
            self.finish_ping(Err(PingError::NoNetwork));
            return;
        }
        STATS.on_ping_sent();
        self.ping = PingJob::Waiting(Ping::new(
            target,
            self.ping_id,
            self.ping_seq,
            sent,
            timeout_ms,
        ));
    }

    pub(super) fn start_ping(&mut self, target: Ipv4, timeout_ms: u32, now: u64) {
        let Some(cfg) = self.net.config().copied() else {
            return self.finish_ping(Err(PingError::NoNetwork));
        };
        if !self.net.port().link_up() {
            return self.finish_ping(Err(PingError::NoNetwork));
        }
        let Some(hop) = icmp::next_hop(target, cfg.ip, cfg.prefix, cfg.gateway) else {
            let usable = net::is_usable_unicast(target) && target != cfg.ip;
            return self.finish_ping(Err(if usable {
                PingError::NoRoute
            } else {
                PingError::BadTarget
            }));
        };
        if let Some(m) = self.arp_lookup(hop, now) {
            return self.send_echo(&cfg, target, m, timeout_ms);
        }
        self.ping = PingJob::Arping {
            target,
            hop,
            timeout_ms,
            tries: 0,
            next_try_ms: now,
        };
    }

    pub(super) fn ping_step(&mut self, now: u64) {
        // A new request from another thread.
        if matches!(self.ping, PingJob::Idle)
            && P_STATE
                .compare_exchange(P_REQUESTED, P_RUNNING, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            let target = Ipv4(P_TARGET.load(Ordering::Relaxed).to_be_bytes());
            let timeout = P_TIMEOUT_MS.load(Ordering::Relaxed);
            self.start_ping(target, timeout, now);
        }

        match core::mem::replace(&mut self.ping, PingJob::Idle) {
            PingJob::Idle => {}
            PingJob::Arping {
                target,
                hop,
                timeout_ms,
                mut tries,
                mut next_try_ms,
            } => {
                if let Some(m) = self.arp_lookup(hop, now) {
                    match self.net.config().copied() {
                        Some(cfg) => self.send_echo(&cfg, target, m, timeout_ms),
                        None => self.finish_ping(Err(PingError::NoNetwork)),
                    }
                    return;
                }
                if now >= next_try_ms {
                    if tries >= ARP_TRIES {
                        self.finish_ping(Err(PingError::ArpFailed));
                        return;
                    }
                    tries += 1;
                    next_try_ms = now + ARP_WAIT_MS;
                    if let Some(cfg) = self.net.config().copied() {
                        let mac = self.net.port().mac();
                        let mut frame = [0u8; 64];
                        let n = net::arp_who_has(&mut frame, mac, cfg.ip, hop);
                        self.net.port().send(&frame[..n]);
                    }
                }
                self.ping = PingJob::Arping {
                    target,
                    hop,
                    timeout_ms,
                    tries,
                    next_try_ms,
                };
            }
            PingJob::Waiting(mut p) => match p.poll(now_us()) {
                Some(r) => self.finish_ping(r),
                None => self.ping = PingJob::Waiting(p),
            },
        }
    }
}
