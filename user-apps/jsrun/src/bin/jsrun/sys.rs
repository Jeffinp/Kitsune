//! What the C side asks of the system (`kjs_*`, declared in `libc/libc.c`): output, the clocks and the
//! end of the process. Only integers and pointers cross: this Rust is soft-float, the C uses SSE.

use core::sync::atomic::{AtomicI64, Ordering};
use kitsune_user as sdk;

/// Write `len` bytes at `buf` to fd 1 or 2 (both are the process output). Returns bytes taken or `-errno`.
///
/// # Safety
/// `buf` must be valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kjs_write(fd: i32, buf: *const u8, len: usize) -> i64 {
    if len == 0 {
        return 0;
    }
    // SAFETY: the caller promises `buf` holds `len` bytes.
    let data = unsafe { core::slice::from_raw_parts(buf, len) };
    sdk::write(if fd == 2 { 2 } else { 1 }, &data[..len.min(4096)])
}

/// Milliseconds since 1970 in UTC (system call 9).
#[unsafe(no_mangle)]
pub extern "C" fn kjs_utc_ms() -> i64 {
    sdk::unix_time_ms()
}

/// Milliseconds since boot.
#[unsafe(no_mangle)]
pub extern "C" fn kjs_uptime_ms() -> u64 {
    sdk::uptime_ms()
}

/// Not measured yet.
const UNKNOWN: i64 = i64::MIN;
static TZ_OFFSET: AtomicI64 = AtomicI64::new(UNKNOWN);

/// The user's time zone as seconds east of UTC: the local clock (call 7) minus UTC (call 9), rounded to
/// a quarter of an hour (the two calls are not at the same instant). Measured once per run.
#[unsafe(no_mangle)]
pub extern "C" fn kjs_tz_offset_secs() -> i64 {
    let cached = TZ_OFFSET.load(Ordering::Relaxed);
    if cached != UNKNOWN {
        return cached;
    }
    let utc = sdk::unix_time_ms();
    let off = if utc <= 0 {
        0
    } else {
        let diff = sdk::local_time_secs() - utc / 1000;
        (diff + 450).div_euclid(900) * 900
    };
    TZ_OFFSET.store(off, Ordering::Relaxed);
    off
}

/// End the process.
#[unsafe(no_mangle)]
pub extern "C" fn kjs_exit(code: i32) -> ! {
    sdk::exit(code)
}
