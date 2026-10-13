//! Intel AC'97 audio (ICH and compatibles; QEMU `-device AC97`): register map, buffer-descriptor
//! encoding and the bookkeeping of the output ring, testable on the host. The port I/O is in
//! `kernel/src/ac97.rs`.
//!
//! The controller has two I/O spaces: the codec's mixer registers (NAM, BAR0) and the bus master
//! (NABM, BAR1) that moves PCM out of memory. The bus master reads a list of 32 buffer descriptors
//! (address, length in 16-bit samples, flags), plays them in order from the current index (`CIV`)
//! up to the last valid index (`LVI`), and stops with `DCH` set when it finishes the last valid one.
//! The driver keeps the descriptors fixed (block `i` is always the same piece of memory) and only
//! fills blocks ahead of the hardware and moves `LVI`; a block is never rewritten while it may still
//! be played, so a late driver gives silence (the device halts), never a stale repeat.
//!
//! [`PlayRing`] turns the registers it reads (`CIV`, `PICB`, `SR`) into the number of frames
//! played since the start, which is the mixer's device clock.

/// PCI ids of controllers that speak this interface (QEMU's is the 82801AA, `8086:2415`).
pub const PCI_IDS: &[(u16, u16)] = &[
    (0x8086, 0x2415), // 82801AA (ICH), QEMU
    (0x8086, 0x2425), // 82801AB (ICH0)
    (0x8086, 0x2445), // 82801BA (ICH2)
    (0x8086, 0x2485), // ICH3
    (0x8086, 0x24C5), // ICH4
    (0x8086, 0x24D5), // ICH5
    (0x8086, 0x266E), // ICH6
    (0x8086, 0x27DE), // ICH7
    (0x8086, 0x7195), // 440MX
    (0x1022, 0x746D), // AMD 8111
    (0x10DE, 0x01B1), // nForce
];

/// Mixer (NAM) registers, 16-bit.
pub const NAM_RESET: u16 = 0x00;
pub const NAM_MASTER: u16 = 0x02;
pub const NAM_PCM_OUT: u16 = 0x18;
pub const NAM_EXT_ID: u16 = 0x28;
pub const NAM_EXT_CTRL: u16 = 0x2A;
pub const NAM_FRONT_RATE: u16 = 0x2C;
/// Bit 15 of a volume register mutes it; 0 attenuation is 0 dB (PCM out: 0x08 per side is 0 dB).
pub const VOL_MUTE: u16 = 0x8000;
pub const MASTER_0DB: u16 = 0x0000;
pub const PCM_0DB: u16 = 0x0808;
/// Variable rate audio: `EXT_ID` says it exists, `EXT_CTRL` turns it on.
pub const EXT_VRA: u16 = 1;

/// Bus master (NABM) registers of the PCM-out box.
pub const PO_BDBAR: u16 = 0x10;
pub const PO_CIV: u16 = 0x14;
pub const PO_LVI: u16 = 0x15;
pub const PO_SR: u16 = 0x16;
pub const PO_PICB: u16 = 0x18;
pub const PO_PIV: u16 = 0x1A;
pub const PO_CR: u16 = 0x1B;
pub const GLOB_CNT: u16 = 0x2C;
pub const GLOB_STA: u16 = 0x30;

/// `CR`: run, reset the box, interrupt enables (left off: the driver polls).
pub const CR_RPBM: u8 = 1;
pub const CR_RR: u8 = 2;
/// `SR`: DMA halted, current = last valid, last valid done, block done, FIFO error. The last three
/// are cleared by writing 1.
pub const SR_DCH: u16 = 1;
pub const SR_CELV: u16 = 2;
pub const SR_LVBCI: u16 = 4;
pub const SR_BCIS: u16 = 8;
pub const SR_FIFOE: u16 = 16;
pub const SR_CLEAR: u16 = SR_LVBCI | SR_BCIS | SR_FIFOE;
/// `GLOB_CNT` bit 1 takes the link out of cold reset; `GLOB_STA` bit 8 is "primary codec ready".
pub const GLOB_CNT_COLD: u32 = 1 << 1;
pub const GLOB_STA_READY: u32 = 1 << 8;

/// Descriptors in the list.
pub const ENTRIES: usize = 32;
/// Descriptor flags: interrupt on completion, and play silence (not the last sample) on underrun.
pub const BD_IOC: u16 = 1 << 15;
pub const BD_BUP: u16 = 1 << 14;

/// One buffer descriptor: 32-bit physical address, length in 16-bit samples, flags.
pub fn descriptor(addr: u32, samples: u16, flags: u16) -> [u8; 8] {
    let mut d = [0u8; 8];
    d[0..4].copy_from_slice(&addr.to_le_bytes());
    d[4..6].copy_from_slice(&samples.to_le_bytes());
    d[6..8].copy_from_slice(&flags.to_le_bytes());
    d
}

/// What [`PlayRing::observe`] learned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observed {
    /// Frames played since the start.
    pub played: u64,
    /// The device ran out of blocks since the last look (it halted after the last valid one).
    pub ran_dry: bool,
}

/// The driver's view of the output ring: blocks handed to the device and blocks it has finished.
///
/// Block `n` (counting from the start) lives in descriptor `n % 32`; the device starts at
/// descriptor 0 and, after a halt, resumes at the one after the last valid, which is always the next
/// block to fill. So `submitted % 32` is both where the driver writes next and where a halted
/// device restarts.
#[derive(Clone, Debug)]
pub struct PlayRing {
    block_frames: u32,
    submitted: u64,
    completed: u64,
    last_civ: u8,
    /// The device was told to run (and has not been seen halted since).
    running: bool,
}

impl PlayRing {
    pub fn new(block_frames: u32) -> PlayRing {
        PlayRing {
            block_frames: block_frames.max(1),
            submitted: 0,
            completed: 0,
            last_civ: 0,
            running: false,
        }
    }

    /// Blocks handed to the device and not finished yet (the current one included).
    pub fn queued(&self) -> u64 {
        self.submitted - self.completed
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// The descriptor the next block goes to.
    pub fn next_index(&self) -> u8 {
        (self.submitted % ENTRIES as u64) as u8
    }

    /// How many blocks to fill now to have `target` queued (never all 32: the block the device is
    /// on, and the one it might be reading ahead, stay untouched).
    pub fn wanted(&self, target: u64) -> u64 {
        target.min(ENTRIES as u64 - 2).saturating_sub(self.queued())
    }

    /// A block was written to [`next_index`](Self::next_index): returns the new last valid index
    /// to tell the device.
    pub fn submit(&mut self) -> u8 {
        let idx = self.next_index();
        self.submitted += 1;
        idx
    }

    /// The device was (re)started after blocks were submitted.
    pub fn started(&mut self) {
        self.running = true;
    }

    /// Update from the registers: `civ` (current index), `picb` (16-bit samples left in the current
    /// block, two per stereo frame) and `sr` (status).
    pub fn observe(&mut self, civ: u8, picb: u16, sr: u16) -> Observed {
        let bf = u64::from(self.block_frames);
        if !self.running || sr & SR_DCH != 0 {
            // Halted: everything handed out has been played, and it resumes at the next block.
            let ran_dry = self.running && sr & SR_DCH != 0;
            self.running = false;
            self.completed = self.submitted;
            self.last_civ = self.next_index();
            return Observed {
                played: self.completed * bf,
                ran_dry,
            };
        }
        let civ = civ % ENTRIES as u8;
        let advanced = u64::from(civ.wrapping_sub(self.last_civ) % ENTRIES as u8);
        self.completed = (self.completed + advanced).min(self.submitted);
        self.last_civ = civ;
        let in_block = if self.completed < self.submitted {
            bf.saturating_sub(u64::from(picb) / 2)
        } else {
            0
        };
        Observed {
            played: self.completed * bf + in_block,
            ran_dry: false,
        }
    }
}

#[cfg(test)]
mod tests;
