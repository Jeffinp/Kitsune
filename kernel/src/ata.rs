//! ATA PIO (28-bit LBA) driver for the filesystem disk.
//!
//! Targets the **secondary IDE channel master** (ports `0x170`/`0x376`), kept
//! separate from the boot disk on the primary channel. Polled (no IRQ/DMA).
//!
//! Two faces over one transfer engine:
//!
//! * [`AtaDisk`] implements [`BlockDevice`] (arbitrary LBA, any number of sectors,
//!   `FLUSH CACHE`), which is what OJFS v3 mounts on. Transfers are sliced into
//!   commands of at most 255 sectors, and the driver **yields the CPU between
//!   sectors** (and now and then inside long waits) so a multi-MiB transfer does
//!   not freeze the compositor.
//! * [`read_image`]/[`write_image`] are the legacy whole-image calls the OJFS v2
//!   desktop still uses (99 sectors at LBA 0). They never yield, exactly as before.
//!
//! Every wait is bounded: a missing or wedged drive produces an error instead of a
//! hang, so callers can fall back to a RAM-only filesystem. After a few consecutive
//! timeouts the controller is declared dead and every call fails at once (a wedged
//! disk must not cost the compositor seconds per attempt). The ATA ports are
//! guarded by their own [`YieldMutex`]: never two threads in the controller.
//! Arithmetic (slicing, bounds, status decoding) lives in `kitsune_core::hw::ata`.

use crate::io::{inb, inw, outb};
use crate::sync::YieldMutex;
use core::arch::asm;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use kitsune_core::blockdev::{BlockDevice, IoError};

const BASE: u16 = 0x170; // secondary channel I/O base
const CTRL: u16 = 0x376; // secondary channel control / alternate status

const REG_DATA: u16 = BASE;
const REG_SECCOUNT: u16 = BASE + 2;
const REG_LBA0: u16 = BASE + 3;
const REG_LBA1: u16 = BASE + 4;
const REG_LBA2: u16 = BASE + 5;
const REG_DRIVE: u16 = BASE + 6;
const REG_STATUS: u16 = BASE + 7;
const REG_CMD: u16 = BASE + 7;

/// Device control: software reset.
const CTRL_SRST: u8 = 0x04;

const CMD_READ: u8 = 0x20;
const CMD_WRITE: u8 = 0x30;
const CMD_FLUSH: u8 = 0xE7;
const CMD_IDENTIFY: u8 = 0xEC;

/// Standard IDE channels: `(io_base, control, label)`. In our QEMU setup the
/// primary master is the boot/OS disk and the secondary master holds the
/// filesystem image (see `run.ps1`).
pub const CHANNELS: [(u16, u16, &str); 2] = [
    (0x1F0, 0x3F6, "primario (boot)"),
    (0x170, 0x376, "secundario (FS)"),
];

pub use kitsune_core::hw::ata::DiskInfo;
use kitsune_core::hw::ata::{
    SECTOR, SR_BSY, SR_DRQ, SR_ERR, Status, check_transfer, chunks, classify_status, lba28_regs,
    parse_identify, usable_sectors,
};

/// Bounded poll budget: see `kitsune_core::hw::ata::poll_exhausted` (iterations AND wall time).
struct Budget(u64);

impl Budget {
    fn new() -> Self {
        Self(crate::netd::now_us())
    }

    fn spent(&self, iterations: u32) -> bool {
        let elapsed = if iterations >= kitsune_core::hw::ata::POLL_ITERS {
            crate::netd::now_us().saturating_sub(self.0)
        } else {
            0
        };
        kitsune_core::hw::ata::poll_exhausted(iterations, elapsed)
    }
}

/// While polling for a long time, hand the CPU to other threads this often.
const YIELD_EVERY: u32 = 1024;

/// Consecutive failures after which the controller is declared dead.
const MAX_FAILS: u32 = 3;

/// Serializes every access to the IDE ports (command sequences span many port
/// accesses and yields, so a plain interrupt mask would not do).
static PORTS: YieldMutex<()> = YieldMutex::new(());

/// Consecutive transfer failures (timeouts, faults, absent drive); a success
/// resets it. At [`MAX_FAILS`] calls fail without touching the hardware.
static FAILS: AtomicU32 = AtomicU32::new(0);

/// Bytes the driver moved since boot (successful transfers only), for the activity
/// monitor. Plain relaxed counters: no lock, no allocation, safe to bump anywhere and
/// to read from any thread.
static READ_BYTES: AtomicU64 = AtomicU64::new(0);
static WRITE_BYTES: AtomicU64 = AtomicU64::new(0);

/// Cumulative `(bytes read, bytes written)` through [`AtaDisk`] since boot.
pub fn io_bytes() -> (u64, u64) {
    (
        READ_BYTES.load(Ordering::Relaxed),
        WRITE_BYTES.load(Ordering::Relaxed),
    )
}

/// Why one controller step failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fail {
    /// The bus floats (`0xFF`): no drive.
    Absent,
    /// ERR or DF in the status register.
    Fault,
    /// The drive stayed busy or never raised DRQ within the budget.
    Timeout,
}

/// ~400ns settle: the spec says read the alternate status four times after
/// selecting a drive or issuing a command before trusting BSY.
fn settle() {
    for _ in 0..4 {
        let _ = inb(CTRL);
    }
}

/// Poll the status register until `done` accepts a sample. `coop` lets other
/// threads run while we wait (with interrupts off `yield_now` is a no-op).
fn wait(coop: bool, done: impl Fn(Status) -> Option<Result<(), Fail>>) -> Result<(), Fail> {
    let budget = Budget::new();
    let mut i = 0u32;
    while !budget.spent(i) {
        let st = classify_status(inb(REG_STATUS));
        if st == Status::Floating {
            return Err(Fail::Absent);
        }
        if let Some(r) = done(st) {
            return r;
        }
        if coop && i % YIELD_EVERY == YIELD_EVERY - 1 {
            crate::sched::yield_now();
        }
        i += 1;
    }
    Err(Fail::Timeout)
}

/// BSY clear (any other state is fine: the next command resets the rest).
fn wait_not_busy(coop: bool) -> Result<(), Fail> {
    wait(coop, |st| (st != Status::Busy).then_some(Ok(())))
}

/// Wait until the drive is ready to transfer a data word (DRQ set, BSY clear).
fn wait_drq(coop: bool) -> Result<(), Fail> {
    wait(coop, |st| match st {
        Status::Drq => Some(Ok(())),
        Status::Fault => Some(Err(Fail::Fault)),
        _ => None,
    })
}

/// Wait until the command is complete: BSY and DRQ clear, no error.
fn wait_done(coop: bool) -> Result<(), Fail> {
    wait(coop, |st| match st {
        Status::Ready => Some(Ok(())),
        Status::Fault => Some(Err(Fail::Fault)),
        _ => None,
    })
}

/// Select the master drive in LBA mode and program the starting LBA + count.
fn setup(lba: u32, sectors: u8, coop: bool) -> Result<(), Fail> {
    wait_not_busy(coop)?;
    let [drive, lba0, lba1, lba2] = lba28_regs(lba);
    outb(REG_DRIVE, drive);
    settle();
    outb(REG_SECCOUNT, sectors);
    outb(REG_LBA0, lba0);
    outb(REG_LBA1, lba1);
    outb(REG_LBA2, lba2);
    Ok(())
}

/// Read one sector's 256 words into `buf`.
fn read_sector_data(buf: &mut [u8; SECTOR]) {
    // SAFETY: `rep insw` stores `rcx` 16-bit words starting at `rdi`. `buf` is a valid, writable,
    // exclusive 512-byte slice and rcx = 256 words = 512 bytes, so the stores stay inside it.
    // The data port is the drive's; DF is clear on entry to inline asm (Rust ABI), so rdi counts up.
    unsafe {
        asm!(
            "rep insw",
            inout("rcx") SECTOR / 2 => _,
            inout("rdi") buf.as_mut_ptr() => _,
            in("dx") REG_DATA,
            options(nostack, preserves_flags),
        );
    }
}

/// Write one sector's 256 words from `buf`.
fn write_sector_data(buf: &[u8; SECTOR]) {
    // SAFETY: `rep outsw` loads `rcx` 16-bit words starting at `rsi`. `buf` is a valid 512-byte
    // slice and rcx = 256 words = 512 bytes, so the loads stay inside it; it only reads memory.
    // DF is clear on entry to inline asm (Rust ABI), so rsi counts up.
    unsafe {
        asm!(
            "rep outsw",
            inout("rcx") SECTOR / 2 => _,
            inout("rsi") buf.as_ptr() => _,
            in("dx") REG_DATA,
            options(nostack, readonly, preserves_flags),
        );
    }
}

/// Software-reset the channel after a failed command so a drive stuck mid-transfer
/// (DRQ up, waiting for data) goes back to idle. Bounded; best effort.
fn soft_reset() {
    outb(CTRL, CTRL_SRST);
    for _ in 0..400 {
        let _ = inb(CTRL); // >= 5 us
    }
    outb(CTRL, 0);
    for _ in 0..20_000 {
        let _ = inb(CTRL); // >= 2 ms for the drive to come back
    }
    let _ = wait_not_busy(false);
}

/// Book-keeping after a controller operation: count consecutive failures and
/// reset the channel when a command died half-way.
fn settle_result(r: Result<(), Fail>) -> Result<(), Fail> {
    match r {
        Ok(()) => FAILS.store(0, Ordering::Relaxed),
        Err(f) => {
            let n = FAILS.fetch_add(1, Ordering::Relaxed) + 1;
            if n == MAX_FAILS {
                DEAD_SINCE_US.store(crate::netd::now_us(), Ordering::Relaxed);
            }
            crate::klog!(
                Warn,
                "ata: transfer failed ({:?}), {} consecutive{}",
                f,
                n,
                if n >= MAX_FAILS {
                    ": controller declared dead"
                } else {
                    ""
                }
            );
            if f != Fail::Absent {
                soft_reset();
            }
        }
    }
    r
}

/// When the controller was declared dead (monotonic microseconds), for the periodic retry.
static DEAD_SINCE_US: AtomicU64 = AtomicU64::new(0);

/// Is the controller declared dead *and* not yet due for another attempt? After
/// `DEAD_RETRY_US` one attempt is let through (the failure counter is set one below the limit, so
/// a single failure declares it dead again and a success clears it).
fn dead() -> bool {
    if FAILS.load(Ordering::Relaxed) < MAX_FAILS {
        return false;
    }
    let now = crate::netd::now_us();
    if kitsune_core::hw::ata::dead_retry_due(DEAD_SINCE_US.load(Ordering::Relaxed), now) {
        DEAD_SINCE_US.store(now, Ordering::Relaxed);
        FAILS.store(MAX_FAILS - 1, Ordering::Relaxed);
        crate::klog!(Info, "ata: trying the controller again");
        return false;
    }
    true
}

/// Sectors moved between two cooperative yields. A PIO sector is a few microseconds of CPU, so
/// yielding after each one made a transfer wait for every other runnable thread's whole turn once
/// per sector (a 16-sector write took ~24 ms with the compositor busy); every 32 keeps the
/// latency other threads see under a tenth of a millisecond.
const SECTORS_PER_YIELD: usize = 32;

/// Read `buf` (a multiple of 512 bytes) starting at `lba`, sliced into commands of
/// at most 255 sectors. The ports lock must be held and the range validated.
fn read_chunks(lba: u64, buf: &mut [u8], coop: bool) -> Result<(), Fail> {
    for c in chunks(lba, buf.len() / SECTOR) {
        setup(c.lba, c.sectors, coop)?;
        outb(REG_CMD, CMD_READ);
        settle();
        let region = &mut buf[c.offset..c.offset + c.sectors as usize * SECTOR];
        for (i, sector) in region.as_chunks_mut::<SECTOR>().0.iter_mut().enumerate() {
            wait_drq(coop)?;
            read_sector_data(sector);
            if coop && (i + 1) % SECTORS_PER_YIELD == 0 {
                crate::sched::yield_now();
            }
        }
        wait_done(coop)?;
    }
    Ok(())
}

/// Write `buf` (a multiple of 512 bytes) starting at `lba`; same contract as [`read_chunks`].
/// Not durable until [`flush_cache`].
fn write_chunks(lba: u64, buf: &[u8], coop: bool) -> Result<(), Fail> {
    for c in chunks(lba, buf.len() / SECTOR) {
        setup(c.lba, c.sectors, coop)?;
        outb(REG_CMD, CMD_WRITE);
        settle();
        let region = &buf[c.offset..c.offset + c.sectors as usize * SECTOR];
        for (i, sector) in region.as_chunks::<SECTOR>().0.iter().enumerate() {
            wait_drq(coop)?;
            write_sector_data(sector);
            if coop && (i + 1) % SECTORS_PER_YIELD == 0 {
                crate::sched::yield_now();
            }
        }
        wait_done(coop)?;
    }
    Ok(())
}

/// `FLUSH CACHE`: everything written so far is on the medium when this returns `Ok`.
fn flush_cache(coop: bool) -> Result<(), Fail> {
    wait_not_busy(coop)?;
    outb(REG_DRIVE, 0xE0);
    settle();
    outb(REG_CMD, CMD_FLUSH);
    settle();
    wait_done(coop)
}

/// A block device over the filesystem disk (secondary IDE master).
///
/// Cheap to copy around: it only remembers the capacity; every call takes the
/// controller lock itself, so several handles (or threads) are safe.
#[derive(Debug)]
pub struct AtaDisk {
    sectors: u64,
}

impl AtaDisk {
    /// Probe the filesystem disk with `IDENTIFY`. `None` if no ATA drive answers.
    /// The capacity is capped at the LBA28 limit (128 GiB).
    pub fn open() -> Option<AtaDisk> {
        let info = identify(BASE, CTRL, false)?;
        Some(AtaDisk {
            sectors: usable_sectors(info.sectors),
        })
    }

    fn account(read: bool, t0: u64) {
        if crate::trace::ON {
            use core::sync::atomic::Ordering::Relaxed;
            let dt = crate::io::rdtsc().wrapping_sub(t0);
            if read {
                crate::trace::ATA_R_N.fetch_add(1, Relaxed);
                crate::trace::ATA_R_CYC.fetch_add(dt, Relaxed);
            } else {
                crate::trace::ATA_W_N.fetch_add(1, Relaxed);
                crate::trace::ATA_W_CYC.fetch_add(dt, Relaxed);
            }
        }
    }
}

impl BlockDevice for AtaDisk {
    fn sector_count(&self) -> u64 {
        self.sectors
    }

    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), IoError> {
        if check_transfer(self.sectors, lba, buf.len())? == 0 {
            return Ok(());
        }
        if dead() {
            return Err(IoError::Read);
        }
        if crate::trace::ON && buf.len() >= 32 * 1024 {
            crate::serial_println!(
                "[trace] ata big read: lba {lba}, {} sectors",
                buf.len() / SECTOR
            );
        }
        let t0 = crate::trace::t();
        let _ports = PORTS.lock().map_err(|_| IoError::Read)?;
        let r = settle_result(read_chunks(lba, buf, true));
        Self::account(true, t0);
        if r.is_ok() {
            READ_BYTES.fetch_add(buf.len() as u64, Ordering::Relaxed);
        }
        r.map_err(|_| IoError::Read)
    }

    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), IoError> {
        if check_transfer(self.sectors, lba, buf.len())? == 0 {
            return Ok(());
        }
        if dead() {
            return Err(IoError::Write);
        }
        let t0 = crate::trace::t();
        let _ports = PORTS.lock().map_err(|_| IoError::Write)?;
        let r = settle_result(write_chunks(lba, buf, true));
        Self::account(false, t0);
        if r.is_ok() {
            WRITE_BYTES.fetch_add(buf.len() as u64, Ordering::Relaxed);
        }
        r.map_err(|_| IoError::Write)
    }

    fn flush(&mut self) -> Result<(), IoError> {
        if dead() {
            return Err(IoError::Flush);
        }
        let _ports = PORTS.lock().map_err(|_| IoError::Flush)?;
        settle_result(flush_cache(true)).map_err(|_| IoError::Flush)
    }
}

/// Bounded wait for BSY to clear on an arbitrary channel's status port.
fn wait_bsy_at(status: u16) -> bool {
    let budget = Budget::new();
    let mut i = 0u32;
    while !budget.spent(i) {
        let s = inb(status);
        if s == 0xFF {
            return false;
        }
        if s & SR_BSY == 0 {
            return true;
        }
        i += 1;
    }
    false
}

/// Run `IDENTIFY DEVICE` on `(base, ctrl)` master/slave and parse the result.
/// `None` when no ATA drive answers (floating bus, ATAPI/SATA signature, or a
/// stalled transfer). Read-only: safe to probe every channel at boot. Takes the
/// ports lock, so it never interleaves with a transfer in another thread.
pub fn identify(base: u16, ctrl: u16, slave: bool) -> Option<DiskInfo> {
    let _ports = PORTS.lock().ok()?;
    let status = base + 7;
    if !wait_bsy_at(status) {
        return None;
    }
    outb(base + 6, if slave { 0xB0 } else { 0xA0 });
    for _ in 0..4 {
        let _ = inb(ctrl);
    }
    // Zero the sector-count/LBA registers, then issue IDENTIFY.
    outb(base + 2, 0);
    outb(base + 3, 0);
    outb(base + 4, 0);
    outb(base + 5, 0);
    outb(base + 7, CMD_IDENTIFY);

    if inb(status) == 0 {
        return None; // no drive on this slot
    }
    if !wait_bsy_at(status) {
        return None;
    }
    // A non-zero LBA1/LBA2 here is an ATAPI/SATA signature, not a plain ATA disk.
    if inb(base + 4) != 0 || inb(base + 5) != 0 {
        return None;
    }
    // Wait for DRQ (or error).
    let mut ok = false;
    let budget = Budget::new();
    let mut i = 0u32;
    while !budget.spent(i) {
        i += 1;
        let s = inb(status);
        if s & SR_ERR != 0 || s == 0xFF {
            return None;
        }
        if s & SR_DRQ != 0 {
            ok = true;
            break;
        }
    }
    if !ok {
        return None;
    }

    let mut id = [0u16; 256];
    for w in id.iter_mut() {
        *w = inw(base);
    }

    Some(parse_identify(&id))
}

/// Probe every standard channel and log a one-line summary per present drive.
/// This is the "detect where the OS is installed + HD vs SSD" report.
pub fn detect_and_log() {
    crate::serial_println!("ATA: scanning disks");
    for (base, ctrl, label) in CHANNELS {
        match identify(base, ctrl, false) {
            Some(d) => {
                let model = d.model_name();
                let kind = if d.ssd {
                    "SSD"
                } else if d.rpm > 0 {
                    "HD"
                } else {
                    "HD/desconhecido"
                };
                crate::serial_println!(
                    "  {} :: {} | {} MiB | {} (rot={})",
                    label,
                    model,
                    d.mib(),
                    kind,
                    d.rpm
                );
            }
            None => crate::serial_println!("  {} :: ausente", label),
        }
    }
}
