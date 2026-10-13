//! Sound: a [`Stream`] of signed 16-bit samples played by the kernel mixer (calls 60–65, see
//! `kitsune_userabi::audio`).
//!
//! ```ignore
//! let s = audio::Stream::open(44_100, 2)?;
//! s.write_all(&pcm_bytes)?;   // waits for room as needed
//! s.drain(u64::MAX);          // until the last sample was heard
//! ```

use crate::abi;
use crate::syscall3;

/// An open stream; closed (immediately) when dropped. Use [`Stream::close_drain`] to let it finish.
pub struct Stream {
    id: u64,
}

/// The raw calls, returning `-errno` on failure.
pub fn open(rate: u32, channels: u32, format: u64) -> i64 {
    // SAFETY: three plain integers.
    unsafe {
        syscall3(
            abi::SYS_AUDIO_OPEN,
            u64::from(rate),
            u64::from(channels),
            format,
        )
    }
}

pub fn write(id: u64, data: &[u8]) -> i64 {
    // SAFETY: the pointer and length describe `data`.
    unsafe {
        syscall3(
            abi::SYS_AUDIO_WRITE,
            id,
            data.as_ptr() as u64,
            data.len() as u64,
        )
    }
}

pub fn status(id: u64) -> Result<abi::AudioStatus, i64> {
    let mut b = [0u8; abi::AUDIO_STATUS_LEN];
    // SAFETY: the kernel writes `AUDIO_STATUS_LEN` bytes into `b`.
    let r = unsafe { syscall3(abi::SYS_AUDIO_STATUS, id, b.as_mut_ptr() as u64, 0) };
    if r < 0 {
        Err(r)
    } else {
        Ok(abi::AudioStatus::from_bytes(&b))
    }
}

pub fn close(id: u64, drain: bool) -> i64 {
    // SAFETY: plain integers.
    unsafe { syscall3(abi::SYS_AUDIO_CLOSE, id, u64::from(drain), 0) }
}

pub fn ctl(id: u64, op: u64, value: u64) -> i64 {
    // SAFETY: plain integers.
    unsafe { syscall3(abi::SYS_AUDIO_CTL, id, op, value) }
}

pub fn wait(id: u64, want: u64, timeout_ms: u64) -> i64 {
    // SAFETY: plain integers.
    unsafe { syscall3(abi::SYS_AUDIO_WAIT, id, want, timeout_ms) }
}

/// The system volume (0–100), or `-errno`.
pub fn master() -> i64 {
    ctl(0, abi::CTL_GET_MASTER, 0)
}

pub fn set_master(volume: u8) -> i64 {
    ctl(0, abi::CTL_SET_MASTER, u64::from(volume))
}

pub fn muted() -> i64 {
    ctl(0, abi::CTL_GET_MUTE, 0)
}

pub fn set_muted(m: bool) -> i64 {
    ctl(0, abi::CTL_SET_MUTE, u64::from(m))
}

impl Stream {
    /// A stream of s16le samples at `rate` with `channels` (1 or 2).
    pub fn open(rate: u32, channels: u32) -> Result<Stream, i64> {
        let r = open(rate, channels, abi::AUDIO_S16LE);
        if r < 0 {
            Err(r)
        } else {
            Ok(Stream { id: r as u64 })
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// Queue what fits: the bytes taken, or `-errno` (`EAGAIN`: full).
    pub fn write(&self, data: &[u8]) -> i64 {
        write(self.id, data)
    }

    /// Queue everything, waiting for room as needed. Returns `Err(-errno)` if the stream failed.
    pub fn write_all(&self, mut data: &[u8]) -> Result<(), i64> {
        let frame_room = data.len().min(abi::MAX_AUDIO_WRITE / 2) as u64;
        while !data.is_empty() {
            let n = self.write(&data[..data.len().min(abi::MAX_AUDIO_WRITE)]);
            if n == abi::errno::EAGAIN || n == 0 {
                // Wait for room for half a write (fewer, bigger copies), at most 1 s per round.
                let r = wait(self.id, frame_room.max(4), 1000);
                if r < 0 {
                    return Err(r);
                }
            } else if n < 0 {
                return Err(n);
            } else {
                data = &data[n as usize..];
            }
        }
        Ok(())
    }

    pub fn status(&self) -> Result<abi::AudioStatus, i64> {
        status(self.id)
    }

    /// Wait until everything written has been played (`true`), or `timeout_ms` passed.
    pub fn drain(&self, timeout_ms: u64) -> bool {
        wait(self.id, 0, timeout_ms) == 1
    }

    /// Wait until `bytes` fit in the queue (`true`) or `timeout_ms` passed.
    pub fn wait_room(&self, bytes: u64, timeout_ms: u64) -> bool {
        wait(self.id, bytes, timeout_ms) == 1
    }

    pub fn set_volume(&self, volume: u8) -> i64 {
        ctl(self.id, abi::CTL_SET_VOLUME, u64::from(volume))
    }

    pub fn set_paused(&self, paused: bool) -> i64 {
        ctl(self.id, abi::CTL_PAUSE, u64::from(paused))
    }

    pub fn flush(&self) -> i64 {
        ctl(self.id, abi::CTL_FLUSH, 0)
    }

    /// Close after what is queued has played (returns at once).
    pub fn close_drain(self) {
        close(self.id, true);
        core::mem::forget(self);
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        close(self.id, false);
    }
}
