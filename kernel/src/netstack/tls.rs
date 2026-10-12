//! TLS plumbing: an embedded-io stream over the smoltcp socket, and the RNG the handshake uses.

use super::*;

// ---- TLS plumbing: an embedded-io stream over the smoltcp socket + an RNG ----

/// A TLS record buffer: the largest record (16 KiB of data) plus its header, type and tag. A buffer of
/// exactly 16 KiB makes every full-size record of a server fail and the page stops short.
pub(super) const TLS_REC: usize = 16 * 1024 + 256;
pub(super) static TLS_RX: RacyCell<[u8; TLS_REC]> = RacyCell::new([0; TLS_REC]);
pub(super) static TLS_TX: RacyCell<[u8; TLS_REC]> = RacyCell::new([0; TLS_REC]);

/// `embedded_io::Read + Write` over the active smoltcp TCP socket. Every call
/// drives the smoltcp poll loop until bytes move, so embedded-tls can run its
/// blocking handshake on our single-threaded stack.
pub(super) struct Stream<'a> {
    pub(super) net: &'a mut Net,
}

#[derive(Debug)]
pub(super) struct StreamError;

impl core::fmt::Display for StreamError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("tcp stream error")
    }
}

impl core::error::Error for StreamError {}

impl embedded_io::Error for StreamError {
    fn kind(&self) -> embedded_io::ErrorKind {
        embedded_io::ErrorKind::Other
    }
}

impl embedded_io::ErrorType for Stream<'_> {
    type Error = StreamError;
}

impl embedded_io::Read for Stream<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, StreamError> {
        let mut end = Net::deadline(12000);
        let hs = HS_DEADLINE.load(Ordering::Relaxed);
        if hs != 0 {
            end = end.min(hs);
        }
        loop {
            self.net.poll();
            let s = self.net.sockets.get_mut::<tcp::Socket>(self.net.tcp);
            if s.can_recv() {
                let n = s.recv_slice(buf).map_err(|_| StreamError)?;
                if n > 0 {
                    return Ok(n);
                }
            }
            // Peer closed and the buffer is drained → EOF.
            if !s.may_recv() && !s.can_recv() {
                return Ok(0);
            }
            if interrupts::ticks() >= end {
                return Err(StreamError);
            }
            core::hint::spin_loop();
        }
    }
}

impl embedded_io::Write for Stream<'_> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, StreamError> {
        let mut end = Net::deadline(12000);
        let hs = HS_DEADLINE.load(Ordering::Relaxed);
        if hs != 0 {
            end = end.min(hs);
        }
        loop {
            self.net.poll();
            let s = self.net.sockets.get_mut::<tcp::Socket>(self.net.tcp);
            if s.can_send() {
                let n = s.send_slice(buf).map_err(|_| StreamError)?;
                if n > 0 {
                    self.net.poll(); // flush the segment out promptly
                    return Ok(n);
                }
            }
            if !s.may_send() {
                return Err(StreamError);
            }
            if interrupts::ticks() >= end {
                return Err(StreamError);
            }
            core::hint::spin_loop();
        }
    }

    fn flush(&mut self) -> Result<(), StreamError> {
        self.net.poll();
        Ok(())
    }
}
