//! TLS plumbing: an embedded-io stream over one pooled smoltcp socket, and the record buffers.

use super::*;
use core::sync::atomic::AtomicPtr;

/// A TLS record buffer: the largest record (16 KiB of data) plus its header, type and tag. A buffer of
/// exactly 16 KiB makes every full-size record of a server fail and the page stops short.
pub(super) const TLS_REC: usize = 16 * 1024 + 256;
/// Two record buffers (read, write) for each pooled connection. Static so they never land on a
/// kernel stack; slot `i` owns buffers `2 * i` and `2 * i + 1` (see `pool::Slot`).
static TLS_BUFS: RacyCell<[[u8; TLS_REC]; 2 * POOL_MAX]> =
    RacyCell::new([[0; TLS_REC]; 2 * POOL_MAX]);

/// The record buffers of pool slot `i`.
///
/// # Safety
/// The caller must make sure no other reference to these buffers is alive: the only user is the TLS
/// connection of slot `i`, and the old one is dropped before a new one is made (`pool::Slot::reset`).
pub(super) unsafe fn tls_buffers(i: usize) -> (&'static mut [u8], &'static mut [u8]) {
    // SAFETY: TLS_BUFS is touched only from the fetcher thread (the only owner of `Net`), slot `i`'s
    // two rows are disjoint from every other slot's, and the caller promises no earlier borrow of
    // them is still alive.
    unsafe {
        let base = TLS_BUFS.get() as *mut [u8; TLS_REC];
        (&mut *base.add(2 * i), &mut *base.add(2 * i + 1))
    }
}

/// The `Net` a stream works on during one TLS operation (null otherwise). Set by [`with_active`]
/// from the `&mut Net` of the caller, which does not touch it until the operation returns.
static ACTIVE: AtomicPtr<Net> = AtomicPtr::new(core::ptr::null_mut());

/// Run one TLS operation `f` with the streams pointed at `net`.
pub(super) fn with_active<R>(net: &mut Net, f: impl FnOnce() -> R) -> R {
    ACTIVE.store(net as *mut Net, Ordering::Release);
    let r = f();
    ACTIVE.store(core::ptr::null_mut(), Ordering::Release);
    r
}

/// The `Net` of the operation in progress.
fn active() -> Result<&'static mut Net, StreamError> {
    let p = ACTIVE.load(Ordering::Acquire);
    if p.is_null() {
        return Err(StreamError);
    }
    // SAFETY: `with_active` stored a pointer made from a live `&mut Net` whose owner does not use it
    // until `f` returns (and clears the pointer); `Net` lives in the fetcher's static and never moves,
    // and only the fetcher thread runs TLS, so this is the only reference in use.
    Ok(unsafe { &mut *p })
}

/// `embedded_io::Read + Write` over pooled TCP socket `sock`. Every call drives the smoltcp poll
/// loop until bytes move, so embedded-tls can run its blocking handshake on our single-threaded
/// stack.
pub(super) struct Stream {
    pub(super) sock: SocketHandle,
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

impl embedded_io::ErrorType for Stream {
    type Error = StreamError;
}

fn stream_deadline() -> u64 {
    let end = Net::deadline(12000);
    match HS_DEADLINE.load(Ordering::Relaxed) {
        0 => end,
        hs => end.min(hs),
    }
}

impl embedded_io::Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, StreamError> {
        let net = active()?;
        let end = stream_deadline();
        loop {
            // What is already in the socket is taken without polling the interface first (a record
            // arrives in several reads); the interface is driven only when the socket is dry.
            let s = net.sockets.get_mut::<tcp::Socket>(self.sock);
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
            net.poll();
            let s = net.sockets.get_mut::<tcp::Socket>(self.sock);
            if !s.can_recv() {
                net.pause();
            }
        }
    }
}

impl embedded_io::Write for Stream {
    fn write(&mut self, buf: &[u8]) -> Result<usize, StreamError> {
        let net = active()?;
        let end = stream_deadline();
        loop {
            net.poll();
            let s = net.sockets.get_mut::<tcp::Socket>(self.sock);
            if s.can_send() {
                let n = s.send_slice(buf).map_err(|_| StreamError)?;
                if n > 0 {
                    net.poll(); // flush the segment out promptly
                    return Ok(n);
                }
            }
            if !s.may_send() {
                return Err(StreamError);
            }
            if interrupts::ticks() >= end {
                return Err(StreamError);
            }
            net.pause();
        }
    }

    fn flush(&mut self) -> Result<(), StreamError> {
        active()?.poll();
        Ok(())
    }
}
