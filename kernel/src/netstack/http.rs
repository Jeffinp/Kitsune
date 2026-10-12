//! Plain and TLS HTTP GETs over the smoltcp socket (`impl Net`, continued from `mod.rs`).

use super::tls::{Stream, TLS_REC, TLS_RX, TLS_TX};
use super::*;

impl Net {
    /// Blocking HTTP/1.1 GET over plain TCP. Returns the raw response bytes,
    /// at most [`MAX_RESPONSE_BYTES`] of them (the same cap as the TLS path).
    pub fn http_get(
        &mut self,
        host: &str,
        path: &str,
        port: u16,
        cap: usize,
        body: Option<&[u8]>,
    ) -> Result<Fetched, FailReason> {
        let ip = self.resolve_or_fail(host)?;
        self.connect(ip, port)?;

        // Request.
        let mut req = Vec::new();
        build_request(&mut req, host, path, port, false, body);
        {
            let s = self.sockets.get_mut::<tcp::Socket>(self.tcp);
            if s.send_slice(&req).is_err() {
                s.abort();
                return Err(FailReason::Network);
            }
        }

        // Drain the response until the peer closes or the cap is reached.
        let mut out = Vec::new();
        let mut truncated = false;
        let end = Self::deadline(10000);
        let mut timed_out = true;
        while interrupts::ticks() < end {
            self.pump();
            let s = self.sockets.get_mut::<tcp::Socket>(self.tcp);
            if s.can_recv() {
                let _ = s.recv(|data| {
                    // Consume everything the socket holds (so it keeps
                    // draining) but keep only what fits under the cap.
                    truncated |= append_capped(&mut out, data, cap);
                    (data.len(), ())
                });
            }
            // Done when the cap is hit or the peer closed and everything it sent was
            // read (CloseWait still counts as `is_active`, so that alone would wait for
            // the whole timeout after the server's FIN).
            if truncated || (!s.may_recv() && !s.can_recv()) {
                timed_out = false;
                break;
            }
        }
        self.abort_conn();
        if truncated {
            crate::serial_println!("http: response truncated at {} bytes", cap);
        }
        if out.is_empty() {
            return Err(if timed_out {
                FailReason::Timeout
            } else {
                FailReason::Network
            });
        }
        Ok(Fetched {
            data: out,
            truncated,
            cert: None,
        })
    }

    /// Open the TCP connection to `ip:port` (bounded). Shared by the plain-HTTP
    /// and TLS paths. An RST during the handshake is [`FailReason::Refused`], no
    /// answer within 8 s is [`FailReason::Timeout`].
    fn connect(&mut self, ip: IpAddress, port: u16) -> Result<(), FailReason> {
        let local_port = 49152 + (crate::rng::u32() as u16 & 0x3FFF);
        {
            let s = self.sockets.get_mut::<tcp::Socket>(self.tcp);
            s.abort();
            if s.connect(self.iface.context(), (ip, port), local_port)
                .is_err()
            {
                return Err(FailReason::Network);
            }
        }
        let end = Self::deadline(8000);
        while interrupts::ticks() < end {
            self.pump();
            let s = self.sockets.get_mut::<tcp::Socket>(self.tcp);
            if s.may_send() {
                return Ok(());
            }
            if s.state() == tcp::State::Closed {
                return Err(FailReason::Refused);
            }
        }
        self.abort_conn();
        Err(FailReason::Timeout)
    }

    /// Blocking HTTPS GET over TLS 1.3 **with certificate verification**.
    /// Returns the raw HTTP response (headers + body) as received inside the TLS
    /// tunnel, capped at [`MAX_RESPONSE_BYTES`] like [`Net::http_get`], and how
    /// the connection was authenticated.
    ///
    /// The server's chain is validated against the embedded trust store, for
    /// `host`, at the trusted time (`crate::tlsv`). A failure aborts the
    /// handshake with [`FailReason::Cert`] unless `allow_insecure` (the user's
    /// per-origin, per-session override) is set, in which case the page is
    /// returned as [`Conn::Insecure`]. A verified chain is [`Conn::Verified`].
    pub fn https_get(
        &mut self,
        host: &str,
        path: &str,
        port: u16,
        allow_insecure: bool,
        cap: usize,
        body: Option<&[u8]>,
    ) -> Result<(Fetched, Conn), FailReason> {
        // No handshake without a seeded generator: wait (bounded) for 128 credited bits first, and
        // refuse rather than draw the client random and the ephemeral key from a weak generator.
        // Before the DNS lookup and the TCP connect, so no half-open connection waits on us.
        if !crate::rng::wait_ready(crate::rng::TLS_WAIT_MS) {
            return Err(FailReason::Tls);
        }
        let ip = self.resolve_or_fail(host)?;
        self.connect(ip, port)?;

        // 16 KiB record buffers (one TLS frame). Kept in static memory so they
        // never land on the kernel stack.
        // SAFETY: TLS_RX is used only here, and `https_get` runs only on the fetcher thread, one call at
        // a time (`&mut self`), so this is the only live reference; `TLS_REC` is the array length. It
        // dies with `tls` before this fn returns.
        let rx_rec: &mut [u8] =
            unsafe { core::slice::from_raw_parts_mut(TLS_RX.get() as *mut u8, TLS_REC) };
        // SAFETY: as for TLS_RX above (TLS_TX is a distinct static, used only here).
        let tx_rec: &mut [u8] =
            unsafe { core::slice::from_raw_parts_mut(TLS_TX.get() as *mut u8, TLS_REC) };

        let config = TlsConfig::new()
            .enable_rsa_signatures()
            .with_server_name(host);
        let mut verifier = crate::tlsv::Verifier::new(allow_insecure);
        HS_DEADLINE.store(
            interrupts::ticks()
                + kitsune_core::tlsverify::MAX_HANDSHAKE_MS * u64::from(interrupts::TIMER_HZ)
                    / 1000,
            Ordering::Relaxed,
        );
        let hs_start = now_ms();
        let stream = Stream { net: self };
        let mut tls: TlsConnection<Stream, Aes128GcmSha256> =
            TlsConnection::new(stream, rx_rec, tx_rec);

        let provider = crate::tlsv::Provider {
            rng: crate::rng::KernelRng,
            verifier: &mut verifier,
        };
        let opened = tls.open(TlsContext::new(&config, provider));
        if let Err(e) = opened {
            HS_DEADLINE.store(0, Ordering::Relaxed);
            self.abort_conn();
            if let Some(ce) = verifier.failure() {
                return Err(FailReason::Cert(ce));
            }
            crate::serial_println!("https: TLS handshake failed for {}: {:?}", host, e);
            return Err(if now_ms().saturating_sub(hs_start) >= 11_000 {
                FailReason::Timeout
            } else {
                FailReason::Tls
            });
        }
        HS_DEADLINE.store(0, Ordering::Relaxed);
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
            None => {
                // The handshake completed without the verifier concluding: never
                // report that as secure.
                self.abort_conn();
                return Err(FailReason::Tls);
            }
        };

        // Request (HTTP/1.1 with Connection: close; chunked and gzip responses are
        // decoded by `browser::page_body`).
        let mut req = Vec::new();
        build_request(&mut req, host, path, port, true, body);
        use embedded_io::Write as _;
        if tls.write_all(&req).is_err() || tls.flush().is_err() {
            self.abort_conn();
            return Err(FailReason::Network);
        }

        // Drain the decrypted response until the peer closes (read returns 0)
        // or the cap is reached.
        let mut out = Vec::new();
        let mut truncated = false;
        let mut buf = [0u8; 2048];
        loop {
            match tls.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if append_capped(&mut out, &buf[..n], cap) {
                        truncated = true; // cap a runaway page
                        break;
                    }
                }
                Err(e) => {
                    // Includes the peer's close_notify; anything else is worth knowing about.
                    if !matches!(e, embedded_tls::TlsError::ConnectionClosed) {
                        crate::serial_println!(
                            "https: read ended with {:?} after {} bytes",
                            e,
                            out.len()
                        );
                    }
                    break;
                }
            }
        }

        // `tls` is unused past here, so its `&mut self` borrow (via Stream) ends
        // and we can touch the socket again to tear the connection down.
        self.abort_conn();
        if truncated {
            crate::serial_println!("https: response truncated at {} bytes", cap);
        }
        if out.is_empty() {
            Err(FailReason::Network)
        } else {
            let cert = kitsune_core::browser::CertInfo::from_leaf(
                verifier.leaf_der(),
                host,
                chain_len,
                root,
            );
            Ok((
                Fetched {
                    data: out,
                    truncated,
                    cert,
                },
                conn,
            ))
        }
    }
}

/// Build an HTTP/1.1 request (GET, or POST with `body`) asking for the language of the interface (the request itself
/// is `kitsune_core::browser::build_get_request`; shared by the plain and TLS paths).
fn build_request(
    req: &mut Vec<u8>,
    host: &str,
    path: &str,
    port: u16,
    tls: bool,
    body: Option<&[u8]>,
) {
    kitsune_core::browser::build_request(
        req,
        kitsune_core::i18n::lang(),
        host,
        path,
        port,
        tls,
        body,
    );
}
