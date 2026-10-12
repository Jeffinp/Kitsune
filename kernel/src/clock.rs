//! The wall clock behind certificate validity checks.
//!
//! Two layers, both UTC (the displayed clock in `rtc.rs` is local time and is
//! left alone):
//!
//! * a *local* clock: the CMOS RTC date and time read **once** at boot
//!   ([`init`], before any other thread exists, so the index/data port pair is
//!   not shared), then advanced by the monotonic timer;
//! * a *trusted* correction: an SNTP offset ([`apply_sntp`]) measured by the
//!   fetcher against a network time server. Without it, [`confirmed`] is false
//!   and the browser tells the user when a certificate date check failed that the
//!   system time is unconfirmed.
//!
//! The RTC itself is never written.

use crate::io::{inb, outb};
use crate::netd::now_ms;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use kitsune_core::sntp::{Measurement, NtpTs};
use kitsune_core::unixtime::{self, from_rtc_fields};

/// RTC reading at boot as Unix milliseconds (0 = the RTC was unreadable or
/// absurd), and the monotonic ms at which it was taken.
static BASE_UNIX_MS: AtomicU64 = AtomicU64::new(0);
static BASE_MONO_MS: AtomicU64 = AtomicU64::new(0);
static OFFSET_MS: AtomicI64 = AtomicI64::new(0);
static CONFIRMED: AtomicBool = AtomicBool::new(false);

const ADDR: u16 = 0x70;
const DATA: u16 = 0x71;

fn read_reg(reg: u8) -> u8 {
    outb(ADDR, reg);
    inb(DATA)
}

/// Read the RTC date/time as Unix seconds (UTC), or `None` when it does not
/// describe a real date. Call only while nothing else uses the CMOS ports
/// (boot, before the scheduler runs other threads).
fn read_rtc_unix() -> Option<u64> {
    // Wait out an update in progress, then read twice until stable.
    let read = || {
        let mut spins = 0u32;
        while read_reg(0x0A) & 0x80 != 0 && spins < 100_000 {
            spins += 1;
        }
        (
            read_reg(0x00),
            read_reg(0x02),
            read_reg(0x04),
            read_reg(0x07),
            read_reg(0x08),
            read_reg(0x09),
            read_reg(0x32),
            read_reg(0x0B),
        )
    };
    let mut a = read();
    for _ in 0..4 {
        let b = read();
        if a == b {
            break;
        }
        a = b;
    }
    let (s, m, h, d, mo, y, c, regb) = a;
    let bin = regb & 0x04 != 0;
    let dec = |v: u8| {
        if bin {
            v
        } else {
            kitsune_core::hw::rtc::bcd_to_bin(v)
        }
    };
    // The hour register keeps the PM flag in bit 7 in 12-hour mode.
    let hour = if regb & 0x02 != 0 {
        dec(h & 0x7F)
    } else {
        let pm = h & 0x80 != 0;
        let h12 = dec(h & 0x7F) % 12;
        h12 + if pm { 12 } else { 0 }
    };
    from_rtc_fields(dec(s), dec(m), hour, dec(d), dec(mo), dec(y), dec(c))
}

/// Capture the boot-time RTC reading. Must run before the other threads start.
pub fn init() {
    let unix = read_rtc_unix();
    BASE_MONO_MS.store(now_ms(), Ordering::Relaxed);
    match unix {
        Some(u) => {
            BASE_UNIX_MS.store(u * 1000, Ordering::Relaxed);
            let mut buf = [0u8; 19];
            let n = unixtime::DateTime::from_unix(u).format(&mut buf);
            crate::serial_println!(
                "clock: RTC {} UTC (unconfirmed)",
                core::str::from_utf8(&buf[..n]).unwrap_or("?")
            );
        }
        None => crate::serial_println!("clock: RTC unreadable or not a real date"),
    }
}

/// The local (RTC-derived) clock in Unix milliseconds, `None` when the RTC gave
/// nothing usable at boot.
pub fn local_unix_ms() -> Option<u64> {
    let base = BASE_UNIX_MS.load(Ordering::Relaxed);
    if base == 0 {
        return None;
    }
    Some(base + now_ms().saturating_sub(BASE_MONO_MS.load(Ordering::Relaxed)))
}

/// Local clock as an NTP timestamp for the SNTP request (falls back to
/// "monotonic ms since boot" when the RTC is unusable: the *offset* then
/// absorbs the whole difference, which is exactly what SNTP is for).
pub fn local_ntp(nonce: u16) -> NtpTs {
    let ms = local_unix_ms().unwrap_or_else(now_ms);
    NtpTs::from_unix_ms(ms).with_nonce(nonce)
}

/// Corrected Unix seconds for certificate validation; `None` when no plausible
/// date is available (no SNTP and a broken RTC).
pub fn trusted_unix_secs() -> Option<u64> {
    let local = local_unix_ms().unwrap_or_else(now_ms);
    let off = OFFSET_MS.load(Ordering::Relaxed);
    let ms = i128::from(local) + i128::from(off);
    let secs = u64::try_from(ms.div_euclid(1000)).ok()?;
    unixtime::is_plausible(secs).then_some(secs)
}

/// Corrected Unix time in milliseconds (system call 9, `Date.now()` of `jsrun`); `None` as for
/// [`trusted_unix_secs`].
pub fn trusted_unix_ms() -> Option<u64> {
    let local = local_unix_ms().unwrap_or_else(now_ms);
    let ms = i128::from(local) + i128::from(OFFSET_MS.load(Ordering::Relaxed));
    let ms = u64::try_from(ms).ok()?;
    unixtime::is_plausible(ms / 1000).then_some(ms)
}

/// True once an SNTP measurement set the offset.
pub fn confirmed() -> bool {
    CONFIRMED.load(Ordering::Acquire)
}

/// Adopt an SNTP measurement: the offset is relative to [`local_ntp`]'s clock.
pub fn apply_sntp(m: &Measurement) {
    OFFSET_MS.store(m.offset_ms, Ordering::Relaxed);
    CONFIRMED.store(true, Ordering::Release);
    if let Some(secs) = trusted_unix_secs() {
        let mut buf = [0u8; 19];
        let n = unixtime::DateTime::from_unix(secs).format(&mut buf);
        crate::serial_println!(
            "sntp: offset {} ms, delay {} ms, stratum {}: time now {} UTC (confirmed)",
            m.offset_ms,
            m.delay_ms,
            m.stratum,
            core::str::from_utf8(&buf[..n]).unwrap_or("?")
        );
    }
}
