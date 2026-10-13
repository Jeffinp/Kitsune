//! Connections kept open between requests (HTTP/1.1 keep-alive), with their TLS session.
//!
//! Up to [`POOL_MAX`] connections, one per smoltcp TCP socket of the `Net` (`Net::tcp[i]`). A
//! connection is keyed by scheme, host, port, the certificate override and the app restriction, and
//! is reused by the next request with the same key while it is idle for less than [`IDLE_MS`] and
//! its socket is still established with nothing unread. Anything odd (an error, a response read
//! until the close, `Connection: close`, an aborted download) closes it. A new connection takes a
//! free slot, else the one idle the longest.
//!
//! The slots live outside `Net` (a TLS connection holds a stream that reaches `Net` through
//! `tls::with_active`, so the two must not overlap); like `Net`, only the fetcher thread uses them.

use super::tls::{Stream, tls_buffers, with_active};
use super::*;
use alloc::boxed::Box;
use alloc::string::String;
use core::sync::atomic::AtomicU32;

/// Most connections open at once.
pub(super) const POOL_MAX: usize = 4;
/// An idle connection older than this is closed instead of reused (ms).
pub(super) const IDLE_MS: u64 = 15_000;

pub(super) type Tls = TlsConnection<'static, Stream, Aes128GcmSha256>;

/// What a connection was opened for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct Key {
    pub(super) https: bool,
    pub(super) host: String,
    pub(super) port: u16,
    /// The person allowed this host past a certificate error.
    pub(super) insecure: bool,
    /// Opened for an app (its address was checked to be public).
    pub(super) public_only: bool,
}

/// One pooled connection.
#[derive(Default)]
pub(super) struct Slot {
    pub(super) key: Option<Key>,
    pub(super) tls: Option<Box<Tls>>,
    pub(super) conn: Option<Conn>,
    pub(super) cert: Option<kitsune_core::browser::CertInfo>,
    /// When the last response on it ended (ms); 0 while a request is on it.
    pub(super) idle_since: u64,
    /// Requests carried so far.
    pub(super) uses: u32,
    /// Something was left unread on it: never reuse.
    pub(super) spoiled: bool,
}

static SLOTS: RacyCell<Vec<Slot>> = RacyCell::new(Vec::new());

/// The pool slots.
pub(super) fn slots() -> &'static mut Vec<Slot> {
    // SAFETY: only the fetcher thread (the owner of `Net`) calls this, one request at a time, and no
    // caller keeps the reference across another call.
    let v = unsafe { &mut *SLOTS.get() };
    if v.is_empty() {
        v.resize_with(POOL_MAX, Slot::default);
    }
    v
}

/// New TCP connections, TLS handshakes and reused connections since boot.
pub static CONNECTS: AtomicU32 = AtomicU32::new(0);
pub static HANDSHAKES: AtomicU32 = AtomicU32::new(0);
pub static REUSES: AtomicU32 = AtomicU32::new(0);

/// `(connections, handshakes, reuses)` since boot.
pub fn stats() -> (u32, u32, u32) {
    (
        CONNECTS.load(Ordering::Relaxed),
        HANDSHAKES.load(Ordering::Relaxed),
        REUSES.load(Ordering::Relaxed),
    )
}

impl Net {
    /// A connection for `target`: an idle one with the same key (`true`), else a new one (`false`).
    /// `fresh` skips reuse (the retry after a reused connection failed).
    pub(super) fn checkout(
        &mut self,
        target: &Target,
        allow_insecure: bool,
        fresh: bool,
    ) -> Result<(usize, bool), FailReason> {
        let key = Key {
            https: target.https,
            host: target.host.clone(),
            port: target.port,
            insecure: allow_insecure,
            public_only: self.public_only,
        };
        self.expire_idle();
        if !fresh
            && let Some(i) = slots()
                .iter()
                .position(|s| s.key.as_ref() == Some(&key) && s.idle_since != 0)
        {
            if self.alive(i) {
                let s = &mut slots()[i];
                s.idle_since = 0;
                s.uses += 1;
                REUSES.fetch_add(1, Ordering::Relaxed);
                crate::serial_println!(
                    "net: reusing connection {} to {}:{} (request {} on it)",
                    i,
                    key.host,
                    key.port,
                    s.uses
                );
                return Ok((i, true));
            }
            self.close_slot(i);
        }
        // A free slot, else the one idle the longest.
        let i = slots()
            .iter()
            .position(|s| s.key.is_none())
            .or_else(|| {
                slots()
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.idle_since != 0)
                    .min_by_key(|(_, s)| s.idle_since)
                    .map(|(i, _)| i)
            })
            .unwrap_or(0);
        self.close_slot(i);
        self.open_slot(i, key)?;
        Ok((i, false))
    }

    /// Close the idle connections that waited too long (also called while the fetcher is idle).
    pub fn expire_idle(&mut self) {
        let now = now_ms();
        for i in 0..POOL_MAX {
            let s = &slots()[i];
            if s.key.is_some() && s.idle_since != 0 && now.saturating_sub(s.idle_since) > IDLE_MS {
                self.close_slot(i);
            }
        }
    }

    /// Is the idle connection in slot `i` still usable (established, nothing unread)?
    fn alive(&mut self, i: usize) -> bool {
        self.poll();
        let s = self.sockets.get_mut::<tcp::Socket>(self.tcp[i]);
        s.state() == tcp::State::Established && s.may_send() && !s.can_recv()
    }

    /// The request in slot `i` ended: keep the connection for the next one or close it.
    pub(super) fn release(&mut self, i: usize, reusable: bool) {
        if reusable && !slots()[i].spoiled {
            slots()[i].idle_since = now_ms().max(1);
            // Let the last ACKs go out.
            self.poll();
        } else {
            self.close_slot(i);
        }
    }

    /// Tear down the connection in slot `i` (if any).
    pub(super) fn close_slot(&mut self, i: usize) {
        let s = &mut slots()[i];
        let had = s.key.is_some();
        // The TLS connection (and its borrow of the slot's record buffers) goes first.
        s.tls = None;
        *s = Slot::default();
        if had {
            self.abort_conn(self.tcp[i]);
        }
    }

    /// Close every pooled connection (the address changed).
    pub(super) fn close_all(&mut self) {
        for i in 0..POOL_MAX {
            self.close_slot(i);
        }
    }

    /// Connect slot `i` for `key`: DNS, TCP and, for https, the TLS handshake with certificate
    /// verification (see [`Net::handshake`]).
    fn open_slot(&mut self, i: usize, key: Key) -> Result<(), FailReason> {
        if key.https && !crate::rng::wait_ready(crate::rng::TLS_WAIT_MS) {
            // No handshake without a seeded generator: refuse rather than draw the client random and
            // the ephemeral key from a weak generator.
            return Err(FailReason::Tls);
        }
        let ip = self.resolve_or_fail(&key.host)?;
        self.connect(self.tcp[i], ip, key.port)?;
        let n = CONNECTS.fetch_add(1, Ordering::Relaxed) + 1;
        if key.https {
            if let Err(e) = self.handshake(i, &key) {
                self.close_slot(i);
                return Err(e);
            }
        } else {
            slots()[i].conn = Some(Conn::Plain);
        }
        crate::serial_println!(
            "net: new connection {} to {}:{} ({} so far, {} TLS handshakes, {} reuses)",
            i,
            key.host,
            key.port,
            n,
            HANDSHAKES.load(Ordering::Relaxed),
            REUSES.load(Ordering::Relaxed)
        );
        let s = &mut slots()[i];
        s.key = Some(key);
        s.uses = 1;
        s.idle_since = 0;
        Ok(())
    }

    /// TLS 1.3 handshake on slot `i` **with certificate verification**: the chain is validated against
    /// the embedded trust store, for the host, at the trusted time (`crate::tlsv`). A failure is
    /// [`FailReason::Cert`] unless the person allowed this host (`key.insecure`), in which case the
    /// connection is [`Conn::Insecure`]; a verified chain is [`Conn::Verified`].
    fn handshake(&mut self, i: usize, key: &Key) -> Result<(), FailReason> {
        let host = key.host.as_str();
        let config = TlsConfig::new()
            .enable_rsa_signatures()
            .with_server_name(host);
        let mut verifier = crate::tlsv::Verifier::new(key.insecure);
        HS_DEADLINE.store(
            interrupts::ticks()
                + kitsune_core::tlsverify::MAX_HANDSHAKE_MS * u64::from(interrupts::TIMER_HZ)
                    / 1000,
            Ordering::Relaxed,
        );
        let hs_start = now_ms();
        // SAFETY: slot `i` was just closed (`close_slot` dropped its TLS connection), so nothing else
        // borrows its record buffers; the new connection below is their only user.
        let (rx, tx) = unsafe { tls_buffers(i) };
        let mut tls: Box<Tls> = Box::new(TlsConnection::new(Stream { sock: self.tcp[i] }, rx, tx));
        let provider = crate::tlsv::Provider {
            rng: crate::rng::KernelRng,
            verifier: &mut verifier,
        };
        let opened = with_active(self, || tls.open(TlsContext::new(&config, provider)));
        HS_DEADLINE.store(0, Ordering::Relaxed);
        if let Err(e) = opened {
            drop(tls);
            if let Some(ce) = verifier.failure() {
                crate::serial_println!("tls: certificate refused for {}: {}", host, ce.reason());
                return Err(FailReason::Cert(ce));
            }
            crate::serial_println!("https: TLS handshake failed for {}: {:?}", host, e);
            return Err(if now_ms().saturating_sub(hs_start) >= 11_000 {
                FailReason::Timeout
            } else {
                FailReason::Tls
            });
        }
        HANDSHAKES.fetch_add(1, Ordering::Relaxed);
        let hs_ms = now_ms().saturating_sub(hs_start);
        let (mut chain_len, mut root) = (1usize, None);
        let conn = match verifier.outcome() {
            Some(crate::tlsv::Outcome::Verified(v)) => {
                chain_len = v.chain_len;
                root = Some(verifier.root_name(&v));
                crate::serial_println!(
                    "tls: chain verified for {} ({} certs, root {}); handshake {} ms",
                    host,
                    v.chain_len,
                    verifier.root_name(&v),
                    hs_ms
                );
                Conn::Verified
            }
            Some(crate::tlsv::Outcome::Overridden(e)) => {
                crate::serial_println!(
                    "tls: UNVERIFIED connection to {} ({}), user override; handshake {} ms",
                    host,
                    e.reason(),
                    hs_ms
                );
                Conn::Insecure
            }
            // The handshake completed without the verifier concluding: never report that as secure.
            None => return Err(FailReason::Tls),
        };
        let cert =
            kitsune_core::browser::CertInfo::from_leaf(verifier.leaf_der(), host, chain_len, root);
        let s = &mut slots()[i];
        s.tls = Some(tls);
        s.conn = Some(conn);
        s.cert = cert;
        Ok(())
    }
}
