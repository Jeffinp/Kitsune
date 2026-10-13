//! Intel AC'97 audio driver (QEMU `-device AC97,audiodev=...`; ICH0–ICH7 and compatibles with I/O
//! BARs), PCM out only, polled (no interrupt line).
//!
//! Why AC'97 first (and not Intel HDA): QEMU emulates it on every host, the Windows build included
//! (`-audiodev dsound`), and its whole output path is two I/O windows, a list of 32 buffer
//! descriptors and one DMA engine: about a tenth of an HDA driver (CORB/RIRB command rings, codec
//! graph walking, stream descriptors). See `docs/design/audio.md`.
//!
//! Memory: 32 blocks of [`BLOCK_FRAMES`] stereo s16 frames (2 KiB each, 64 KiB, in a static so the
//! physical address is stable) and the descriptor list. The descriptors never change; the driver
//! fills blocks ahead of the device and moves the last valid index (`LVI`). It keeps about
//! [`TARGET_BLOCKS`] blocks (~128 ms) queued, refilled by the `audiod` thread every tick (4 ms), so
//! a thread that is late by a hundred milliseconds still does not starve the device. When nothing
//! is playing it stops filling and the device halts after the last block (silence, no CPU). The
//! bookkeeping (what `CIV`/`PICB`/`SR` mean in frames played, when to fill) is
//! `kitsune_core::hw::ac97::PlayRing`, tested on the host.

use crate::io::{inb, inl, inw, outb, outl, outw};
use crate::sync::RacyCell;
use crate::{klog, pci};
use core::sync::atomic::{AtomicBool, Ordering, fence};
use kitsune_core::hw::ac97::{self as hw, PlayRing};
use kitsune_core::hw::audio::{DEVICE_RATE, Frame, Mixer};

/// Frames per block (10.7 ms at 48 kHz).
pub const BLOCK_FRAMES: usize = 512;
const BLOCK_BYTES: usize = BLOCK_FRAMES * 4;
/// Blocks kept queued ahead of the device.
pub const TARGET_BLOCKS: u64 = 12;

#[repr(C, align(4096))]
struct Blocks([u8; hw::ENTRIES * BLOCK_BYTES]);
#[repr(C, align(4096))]
struct List([u8; 4096]);

// DMA memory, owned by the single `Ac97` that `probe` hands out (`TAKEN`).
static BLOCKS: RacyCell<Blocks> = RacyCell::new(Blocks([0; hw::ENTRIES * BLOCK_BYTES]));
static LIST: RacyCell<List> = RacyCell::new(List([0; 4096]));
static TAKEN: AtomicBool = AtomicBool::new(false);

/// A running AC'97 controller.
pub struct Ac97 {
    nam: u16,
    nabm: u16,
    ring: PlayRing,
    blocks: *mut u8,
    /// Times the device ran out of blocks while there was sound waiting (the thread was late).
    pub late: u64,
    pub fifo_errors: u64,
}

// SAFETY: `blocks` points at the static DMA memory that only this value uses (`TAKEN` makes sure
// there is one); it is moved into the audio state once and used under its lock.
unsafe impl Send for Ac97 {}

/// Wait up to `ticks` timer ticks for `ok()`.
fn wait_for(ticks: u64, ok: impl Fn() -> bool) -> bool {
    let end = crate::interrupts::ticks() + ticks;
    while crate::interrupts::ticks() <= end {
        if ok() {
            return true;
        }
        core::hint::spin_loop();
    }
    ok()
}

impl Ac97 {
    /// Find and start the first AC'97 controller. `None` (with a log line saying why when one exists
    /// but cannot be driven) if there is none.
    pub fn probe(phys_offset: u64) -> Option<Ac97> {
        let dev = hw::PCI_IDS.iter().find_map(|&(v, d)| pci::find(v, d))?;
        let (bar0, bar1) = (dev.bar(0), dev.bar(1));
        klog!(
            Info,
            "audio: AC'97 {:04x}:{:04x} at pci {:02x}:{:02x}.{} bar0={:#x} bar1={:#x}",
            dev.vendor,
            dev.device,
            dev.bus,
            dev.slot,
            dev.func,
            bar0,
            bar1
        );
        if bar0 & 1 == 0 || bar1 & 1 == 0 {
            klog!(
                Warn,
                "audio: AC'97 without I/O BARs (memory-mapped only): not supported"
            );
            return None;
        }
        if TAKEN.swap(true, Ordering::AcqRel) {
            return None;
        }
        let nam = (bar0 & 0xFFFC) as u16;
        let nabm = (bar1 & 0xFFFC) as u16;
        // I/O space + bus master.
        let cmd = pci::read32(dev.bus, dev.slot, dev.func, 0x04);
        pci::write32(dev.bus, dev.slot, dev.func, 0x04, cmd | 0x5);

        // Out of cold reset, interrupts off; wait for the codec.
        outl(nabm + hw::GLOB_CNT, hw::GLOB_CNT_COLD);
        if !wait_for(50, || inl(nabm + hw::GLOB_STA) & hw::GLOB_STA_READY != 0) {
            klog!(Warn, "audio: AC'97 codec not ready");
            return None;
        }
        outw(nam + hw::NAM_RESET, 0);
        outw(nam + hw::NAM_MASTER, hw::MASTER_0DB);
        outw(nam + hw::NAM_PCM_OUT, hw::PCM_0DB);
        let mut rate = DEVICE_RATE;
        if inw(nam + hw::NAM_EXT_ID) & hw::EXT_VRA != 0 {
            outw(
                nam + hw::NAM_EXT_CTRL,
                inw(nam + hw::NAM_EXT_CTRL) | hw::EXT_VRA,
            );
            outw(nam + hw::NAM_FRONT_RATE, DEVICE_RATE as u16);
            rate = u32::from(inw(nam + hw::NAM_FRONT_RATE));
        }
        if rate != DEVICE_RATE {
            klog!(
                Warn,
                "audio: codec runs at {rate} Hz, not {DEVICE_RATE}: pitch will be off"
            );
        }

        // Reset the PCM-out box, then point it at the descriptor list.
        outb(nabm + hw::PO_CR, hw::CR_RR);
        if !wait_for(10, || inb(nabm + hw::PO_CR) & hw::CR_RR == 0) {
            klog!(Warn, "audio: AC'97 bus master did not reset");
            return None;
        }
        let blocks = BLOCKS.get() as *mut u8;
        let list = LIST.get() as *mut u8;
        let list_phys = crate::virtio::virt_to_phys(list as u64, phys_offset)?;
        if list_phys > u64::from(u32::MAX) {
            klog!(Warn, "audio: DMA memory above 4 GiB, AC'97 cannot reach it");
            return None;
        }
        for i in 0..hw::ENTRIES {
            // Each block is 2 KiB inside a 4 KiB-aligned static, so it never crosses a page and is
            // physically contiguous on its own.
            let virt = blocks as u64 + (i * BLOCK_BYTES) as u64;
            let phys = crate::virtio::virt_to_phys(virt, phys_offset)?;
            let Ok(phys) = u32::try_from(phys) else {
                klog!(Warn, "audio: DMA memory above 4 GiB, AC'97 cannot reach it");
                return None;
            };
            let d = hw::descriptor(phys, (BLOCK_FRAMES * 2) as u16, hw::BD_BUP);
            // SAFETY: `list` is the 4096-byte LIST static owned by this driver (`TAKEN`); entry `i < 32`
            // spans bytes `8i..8i+8`, inside it. The device is reset and not reading the list yet.
            unsafe { core::ptr::copy_nonoverlapping(d.as_ptr(), list.add(i * 8), 8) };
        }
        fence(Ordering::SeqCst);
        outl(nabm + hw::PO_BDBAR, list_phys as u32);
        klog!(
            Info,
            "audio: AC'97 up: nam={nam:#x} nabm={nabm:#x}, {rate} Hz stereo s16, {} x {BLOCK_FRAMES} frames, ~{} ms ahead",
            hw::ENTRIES,
            TARGET_BLOCKS * BLOCK_FRAMES as u64 * 1000 / u64::from(DEVICE_RATE)
        );
        Some(Ac97 {
            nam,
            nabm,
            ring: PlayRing::new(BLOCK_FRAMES as u32),
            blocks,
            late: 0,
            fifo_errors: 0,
        })
    }

    /// Read where the device is and tell the mixer (its clock). Returns `true` while the device runs.
    pub fn observe(&mut self, mixer: &mut Mixer) -> bool {
        let civ = inb(self.nabm + hw::PO_CIV);
        let picb = inw(self.nabm + hw::PO_PICB);
        let sr = inw(self.nabm + hw::PO_SR);
        if sr & hw::SR_FIFOE != 0 {
            self.fifo_errors += 1;
        }
        if sr & hw::SR_CLEAR != 0 {
            outw(self.nabm + hw::PO_SR, sr & hw::SR_CLEAR);
        }
        let o = self.ring.observe(civ, picb, sr);
        if o.ran_dry && mixer.has_audio() {
            self.late += 1;
        }
        mixer.set_device_played(o.played);
        self.ring.running()
    }

    /// Mix one more block into the ring if the device wants one and there is sound; `false` when
    /// nothing was done.
    pub fn fill_one(&mut self, mixer: &mut Mixer) -> bool {
        if self.ring.wanted(TARGET_BLOCKS) == 0 || !mixer.has_audio() {
            return false;
        }
        let idx = usize::from(self.ring.next_index());
        // SAFETY: block `idx < 32` is bytes `idx * 2048 ..+2048` of the BLOCKS static this driver owns,
        // 4-aligned (so `[i16; 2]` frames are aligned) and not being read by the device: it is past the
        // last valid index (`PlayRing::wanted` never lets the driver reach the block being played).
        let block: &mut [Frame] = unsafe {
            core::slice::from_raw_parts_mut(
                self.blocks.add(idx * BLOCK_BYTES) as *mut Frame,
                BLOCK_FRAMES,
            )
        };
        mixer.mix(block);
        // The samples must be in memory before the device is told it may read them.
        fence(Ordering::SeqCst);
        let lvi = self.ring.submit();
        outb(self.nabm + hw::PO_LVI, lvi);
        if !self.ring.running() {
            if inb(self.nabm + hw::PO_CR) & hw::CR_RPBM == 0 {
                outb(self.nabm + hw::PO_CR, hw::CR_RPBM);
            } else if inw(self.nabm + hw::PO_SR) & hw::SR_DCH != 0 {
                // Halted and the new LVI did not restart it: stop and run again (resumes at the
                // next descriptor, which is the block just written).
                outb(self.nabm + hw::PO_CR, 0);
                outb(self.nabm + hw::PO_CR, hw::CR_RPBM);
            }
            self.ring.started();
        }
        true
    }

    /// Mute or unmute at the codec too (the mixer already outputs zeros when muted; this also
    /// silences the analog stage).
    pub fn set_codec_mute(&self, mute: bool) {
        let v = if mute { hw::VOL_MUTE } else { hw::MASTER_0DB };
        outw(self.nam + hw::NAM_MASTER, v);
    }
}
