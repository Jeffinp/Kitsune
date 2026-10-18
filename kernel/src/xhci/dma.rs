//! The memory the controller reads and writes: device context base array, command and event rings,
//! contexts, per-device transfer rings and buffers. One static block (so physical addresses are stable
//! and nothing is allocated), laid out below; the scratchpad pages the controller asks for live in a
//! second static. Offsets are constants, every object sits inside one 4 KiB page, and all access goes
//! through the bounds-checked accessors here.

use crate::sync::RacyCell;
use core::sync::atomic::{AtomicBool, Ordering, fence};
use kitsune_core::hw::xhci::ring::ProducerRing;
use kitsune_core::hw::xhci::trb::{TRB_BYTES, Trb};

// ---- layout (bytes from the start of the block) ----
/// Device Context Base Address Array: 4 slots + the scratchpad entry would be 40 bytes, but the
/// controller may be told more slots than we use, so the whole 2 KiB (255 slots) is reserved.
pub const DCBAA: usize = 0x0000;
pub const ERST: usize = 0x0800;
pub const SCRATCH_ARRAY: usize = 0x0900;
pub const CMD_RING: usize = 0x1000;
pub const EVENT_RING: usize = 0x1400;
/// Input context, up to 33 contexts of 64 bytes.
pub const INPUT_CONTEXT: usize = 0x2000;
/// Output device contexts, one 2 KiB block per slot (32 contexts of 64 bytes).
pub const DEVICE_CONTEXTS: usize = 0x3000;
pub const DEVICE_CONTEXT_STRIDE: usize = 0x800;
/// Per-device area: EP0 ring, control buffer, two interrupt rings, two report buffers.
pub const DEVICES: usize = 0x6000;
pub const DEVICE_STRIDE: usize = 0x2000;
pub const DMA_SIZE: usize = DEVICES + MAX_DEVICES * DEVICE_STRIDE;

pub const MAX_DEVICES: usize = 4;
/// TRBs per ring (the last one is the link).
pub const RING_TRBS: u16 = 64;
pub const RING_BYTES: usize = RING_TRBS as usize * TRB_BYTES;

// Offsets inside a device area.
pub const DEV_EP0_RING: usize = 0x000;
pub const DEV_CONTROL_BUF: usize = 0x400;
pub const CONTROL_BUF_BYTES: usize = 1024;
pub const DEV_INT_RING: usize = 0x800; // + 0x400 * interface index (0 or 1)
pub const DEV_REPORT_BUF: usize = 0x1000; // + 0x100 * interface index
pub const REPORT_BUF_BYTES: usize = 256;

/// Scratchpad pages we can provide (the controller says how many it needs).
pub const SCRATCH_PAGES: usize = 16;
pub const PAGE: usize = 4096;

#[repr(C, align(4096))]
struct Block([u8; DMA_SIZE]);
#[repr(C, align(4096))]
struct Scratch([u8; SCRATCH_PAGES * PAGE]);

static BLOCK: RacyCell<Block> = RacyCell::new(Block([0; DMA_SIZE]));
static SCRATCH: RacyCell<Scratch> = RacyCell::new(Scratch([0; SCRATCH_PAGES * PAGE]));
static TAKEN: AtomicBool = AtomicBool::new(false);

/// Handle to the DMA block; only one exists (`take`).
pub struct Dma {
    base: u64,
    scratch: u64,
    phys_offset: u64,
}

impl Dma {
    /// The single handle, or `None` if it was already taken.
    pub fn take(phys_offset: u64) -> Option<Dma> {
        if TAKEN.swap(true, Ordering::AcqRel) {
            return None;
        }
        Some(Dma {
            base: BLOCK.get() as u64,
            scratch: SCRATCH.get() as u64,
            phys_offset,
        })
    }

    /// Physical address of the byte at `off` in the block.
    pub fn phys(&self, off: usize) -> Option<u64> {
        if off >= DMA_SIZE {
            return None;
        }
        crate::virtio::virt_to_phys(self.base + off as u64, self.phys_offset)
    }

    /// Physical address of scratchpad page `i`.
    pub fn scratch_phys(&self, i: usize) -> Option<u64> {
        if i >= SCRATCH_PAGES {
            return None;
        }
        crate::virtio::virt_to_phys(self.scratch + (i * PAGE) as u64, self.phys_offset)
    }

    fn ptr(&self, off: usize, size: usize) -> Option<u64> {
        let end = off.checked_add(size)?;
        (end <= DMA_SIZE && off.is_multiple_of(4)).then(|| self.base + off as u64)
    }

    pub fn r32(&self, off: usize) -> u32 {
        match self.ptr(off, 4) {
            // SAFETY: inside the static block, 4-aligned (the block is 4096-aligned and `off % 4 == 0`);
            // volatile because the controller writes this memory.
            Some(p) => unsafe { core::ptr::read_volatile(p as *const u32) },
            None => 0,
        }
    }

    pub fn w32(&self, off: usize, value: u32) {
        if let Some(p) = self.ptr(off, 4) {
            // SAFETY: as in `r32`; volatile because the controller reads this memory.
            unsafe { core::ptr::write_volatile(p as *mut u32, value) }
        }
    }

    pub fn w64(&self, off: usize, value: u64) {
        self.w32(off, value as u32);
        self.w32(off + 4, (value >> 32) as u32);
    }

    pub fn zero(&self, off: usize, len: usize) {
        for o in (off..off.saturating_add(len)).step_by(4) {
            self.w32(o, 0);
        }
    }

    pub fn read_trb(&self, off: usize) -> Trb {
        Trb::from_dwords([
            self.r32(off),
            self.r32(off + 4),
            self.r32(off + 8),
            self.r32(off + 12),
        ])
    }

    /// Writes a TRB. The control dword (which holds the cycle bit) goes last, behind a fence, so the
    /// controller never sees a half-written TRB as valid.
    pub fn write_trb(&self, off: usize, trb: &Trb) {
        let d = trb.to_dwords();
        self.w32(off, d[0]);
        self.w32(off + 4, d[1]);
        self.w32(off + 8, d[2]);
        fence(Ordering::SeqCst);
        self.w32(off + 12, d[3]);
    }

    pub fn write_context(&self, off: usize, context: &[u32; 8]) {
        for (i, &d) in context.iter().enumerate() {
            self.w32(off + i * 4, d);
        }
    }

    /// Copies `len` bytes at `off` into `out` (clamped to both).
    pub fn read_bytes(&self, off: usize, out: &mut [u8]) {
        for (i, b) in out.iter_mut().enumerate() {
            let word = self.r32((off + i) & !3);
            *b = (word >> (((off + i) & 3) * 8)) as u8;
        }
    }
}

/// A producer ring living in the block at `off`.
pub struct Ring {
    pub off: usize,
    pub ring: ProducerRing,
}

impl Ring {
    /// A zeroed ring at `off` (a TRB-aligned offset inside the block).
    pub fn new(dma: &Dma, off: usize) -> Option<Ring> {
        dma.zero(off, RING_BYTES);
        Some(Ring {
            off,
            ring: ProducerRing::new(dma.phys(off)?, RING_TRBS)?,
        })
    }

    /// Queues `trb` and returns the physical address it landed at.
    pub fn push(&mut self, dma: &Dma, trb: Trb) -> u64 {
        let pushed = self.ring.push(trb);
        dma.write_trb(self.off + pushed.index as usize * TRB_BYTES, &pushed.trb);
        if let Some((index, link)) = pushed.link {
            dma.write_trb(self.off + index as usize * TRB_BYTES, &link);
        }
        self.ring.address_of(pushed.index)
    }
}
