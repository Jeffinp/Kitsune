//! One HTTP/1.1 request and its response over a pooled connection (`impl Net`, continued from
//! `mod.rs`). The response is read with `kitsune_core::http1::ResponseReader`, which says where it
//! ends, so the connection can carry the next request (`pool.rs`).

use super::pool::slots;
use super::tls::with_active;
use super::*;
use kitsune_core::http1::{HttpError, Part, Request, ResponseReader, write_request};

/// What a request hands on while its answer arrives.
pub enum Event<'a> {
    /// Response bytes as they came off the connection (headers and body, chunk framing included).
    Raw(&'a [u8]),
    /// The final header block, once.
    Head(&'a [u8]),
    /// Body bytes, chunk framing removed.
    Body(&'a [u8]),
}

/// A request to send with [`Net::exchange`].
pub struct Req<'a> {
    pub method: &'a str,
    pub target: &'a Target,
    /// Headers after `Host` (`Content-Length` and `Connection` are written by the request writer).
    pub headers: &'a [(&'a str, &'a str)],
    pub body: Option<&'a [u8]>,
    /// The person allowed this host past a certificate error.
    pub allow_insecure: bool,
}

/// How an exchange went.
pub struct Exchanged {
    pub conn: Conn,
    pub cert: Option<kitsune_core::browser::CertInfo>,
    /// The response was read to its end (`false`: the connection broke off, or the caller stopped).
    pub complete: bool,
}

enum Fail {
    /// Nothing of the answer arrived: on a reused connection, try once more on a new one.
    Retry,
    Fail(FailReason),
}

/// Feed `data` to `reader`, handing the parts and the raw bytes to `on`. `stop` turns true when
/// `on` asks to stop. Returns the bytes that belong to the response.
fn feed(
    reader: &mut ResponseReader,
    data: &[u8],
    on: &mut dyn FnMut(Event<'_>) -> bool,
    stop: &mut bool,
) -> Result<usize, HttpError> {
    let n = reader.feed(data, &mut |p| {
        if *stop {
            return;
        }
        let go = match p {
            Part::Head(h) => on(Event::Head(h)),
            Part::Body(b) => on(Event::Body(b)),
        };
        *stop |= !go;
    })?;
    if !*stop && n > 0 {
        *stop |= !on(Event::Raw(&data[..n]));
    }
    Ok(n)
}

impl Net {
    /// Send `req` and read its answer, handing it to `on` as it arrives (`on` returns `false` to stop:
    /// the connection is then closed). Uses an idle pooled connection to the same place when there
    /// is one (and retries once on a new one when the server had closed it meanwhile).
    pub fn exchange(
        &mut self,
        req: &Req,
        on: &mut dyn FnMut(Event<'_>) -> bool,
    ) -> Result<Exchanged, FailReason> {
        let mut fresh = false;
        loop {
            let (i, reused) = self.checkout(req.target, req.allow_insecure, fresh)?;
            match self.run_on(i, req, on) {
                Ok((x, reusable)) => {
                    self.release(i, reusable);
                    return Ok(x);
                }
                Err(Fail::Retry) if reused && !fresh => {
                    crate::serial_println!(
                        "net: connection {} was closed by the server; trying a new one",
                        i
                    );
                    self.close_slot(i);
                    fresh = true;
                }
                Err(Fail::Retry) => {
                    self.close_slot(i);
                    return Err(FailReason::Network);
                }
                Err(Fail::Fail(e)) => {
                    self.close_slot(i);
                    return Err(e);
                }
            }
        }
    }

    /// One request on the connection in slot `i`; also says whether the connection may be reused.
    fn run_on(
        &mut self,
        i: usize,
        req: &Req,
        on: &mut dyn FnMut(Event<'_>) -> bool,
    ) -> Result<(Exchanged, bool), Fail> {
        let mut bytes = Vec::with_capacity(512 + req.body.map_or(0, <[u8]>::len));
        write_request(
            &mut bytes,
            &Request {
                method: req.method,
                target: req.target,
                headers: req.headers,
                body: req.body,
                keep_alive: true,
            },
        );
        let is_tls = slots()[i].tls.is_some();
        if is_tls {
            self.send_tls(i, &bytes)?;
        } else {
            self.send_plain(i, &bytes)?;
        }
        drop(bytes);
        let mut reader = ResponseReader::new(req.method == "HEAD");
        let mut stop = false;
        let complete = if is_tls {
            self.read_tls(i, &mut reader, on, &mut stop)?
        } else {
            self.read_plain(i, &mut reader, on, &mut stop)?
        };
        let s = &slots()[i];
        let x = Exchanged {
            conn: s.conn.unwrap_or(Conn::Plain),
            cert: s.cert.clone(),
            complete: complete && !stop,
        };
        Ok((x, complete && !stop && reader.reusable()))
    }

    fn send_plain(&mut self, i: usize, bytes: &[u8]) -> Result<(), Fail> {
        let h = self.tcp[i];
        let mut at = 0;
        let end = Self::deadline(12000);
        while at < bytes.len() {
            self.poll();
            let s = self.sockets.get_mut::<tcp::Socket>(h);
            if !s.may_send() {
                return Err(Fail::Retry);
            }
            match s.send_slice(&bytes[at..]) {
                Ok(n) if n > 0 => at += n,
                Ok(_) => {
                    if interrupts::ticks() >= end {
                        return Err(Fail::Fail(FailReason::Timeout));
                    }
                    self.pause();
                }
                Err(_) => return Err(Fail::Retry),
            }
        }
        self.poll();
        Ok(())
    }

    fn send_tls(&mut self, i: usize, bytes: &[u8]) -> Result<(), Fail> {
        use embedded_io::Write as _;
        let r = with_active(self, || {
            let tls = slots()[i].tls.as_mut().ok_or(())?;
            tls.write_all(bytes).map_err(|_| ())?;
            tls.flush().map_err(|_| ())
        });
        r.map_err(|_| Fail::Retry)
    }

    /// Read the answer on a plain connection; `Ok(true)` when it arrived whole.
    fn read_plain(
        &mut self,
        i: usize,
        reader: &mut ResponseReader,
        on: &mut dyn FnMut(Event<'_>) -> bool,
        stop: &mut bool,
    ) -> Result<bool, Fail> {
        let h = self.tcp[i];
        let mut end = Self::deadline(12000);
        loop {
            let s = self.sockets.get_mut::<tcp::Socket>(h);
            if s.can_recv() {
                let r = s.recv(|data| match feed(reader, data, on, stop) {
                    Ok(n) => (n, Ok(())),
                    Err(e) => (data.len(), Err(e)),
                });
                match r {
                    Ok(Ok(())) => end = Self::deadline(12000),
                    Ok(Err(e)) => {
                        crate::serial_println!("http: bad response: {:?}", e);
                        return Err(Fail::Fail(FailReason::Network));
                    }
                    Err(_) => return Err(Fail::Retry),
                }
                if *stop {
                    return Ok(false);
                }
                if reader.is_done() {
                    return Ok(true);
                }
                continue;
            }
            if !s.may_recv() {
                // The server closed and everything it sent was read.
                return match reader.eof() {
                    Ok(()) => Ok(true),
                    Err(_) if !reader.started() => Err(Fail::Retry),
                    Err(_) => Ok(false),
                };
            }
            if interrupts::ticks() >= end {
                return if reader.started() {
                    Ok(false)
                } else {
                    Err(Fail::Fail(FailReason::Timeout))
                };
            }
            self.poll();
            if !self.sockets.get_mut::<tcp::Socket>(h).can_recv() {
                self.pause();
            }
        }
    }

    /// Read the answer on a TLS connection, straight from the decrypted record (no copy).
    fn read_tls(
        &mut self,
        i: usize,
        reader: &mut ResponseReader,
        on: &mut dyn FnMut(Event<'_>) -> bool,
        stop: &mut bool,
    ) -> Result<bool, Fail> {
        loop {
            let r = with_active(self, || {
                let tls = slots()[i].tls.as_mut().ok_or(None::<HttpError>)?;
                match tls.read_buffered() {
                    Ok(mut buf) => {
                        let data = buf.peek_all();
                        let n = feed(reader, data, on, stop).map_err(Some)?;
                        if n < data.len() {
                            // Bytes after the response (the server pipelined something?): this
                            // connection must not carry another request.
                            slots()[i].spoiled = true;
                        }
                        buf.pop(n);
                        Ok(())
                    }
                    Err(e) => {
                        if !matches!(e, embedded_tls::TlsError::ConnectionClosed) {
                            crate::serial_println!("https: read ended with {:?}", e);
                        }
                        Err(None::<HttpError>)
                    }
                }
            });
            match r {
                Ok(()) => {
                    if *stop {
                        return Ok(false);
                    }
                    if reader.is_done() {
                        return Ok(true);
                    }
                }
                Err(Some(e)) => {
                    crate::serial_println!("https: bad response: {:?}", e);
                    return Err(Fail::Fail(FailReason::Network));
                }
                // Closed (close_notify, FIN) or broken off.
                Err(None) => {
                    return match reader.eof() {
                        Ok(()) => Ok(true),
                        Err(_) if !reader.started() => Err(Fail::Retry),
                        Err(_) => Ok(false),
                    };
                }
            }
        }
    }
}
