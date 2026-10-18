//! xHCI (USB 3.x host controller): registers, ports and extended capabilities, decoded without touching
//! hardware. The kernel maps the controller's MMIO window and does the volatile reads and writes; the
//! arithmetic (what a register means, which bits a write may touch, how the rings wrap) lives here so it is
//! tested on the host. Written against the xHCI specification, revision 1.2.
//!
//! - [`trb`]: the 16-byte transfer request blocks and the events the controller returns.
//! - [`ring`]: producer ring (commands, transfers) and consumer ring (events) bookkeeping.
//! - [`context`]: device, slot and endpoint contexts the controller reads to address a device.

pub mod context;
pub mod ring;
pub mod trb;

#[cfg(test)]
mod tests;

// ---- capability registers (offsets from the MMIO base) ----

pub const CAP_LENGTH_VERSION: usize = 0x00;
pub const CAP_HCSPARAMS1: usize = 0x04;
pub const CAP_HCSPARAMS2: usize = 0x08;
pub const CAP_HCCPARAMS1: usize = 0x10;
pub const CAP_DBOFF: usize = 0x14;
pub const CAP_RTSOFF: usize = 0x18;

/// Decoded capability registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// Offset of the operational registers from the MMIO base.
    pub cap_length: u8,
    /// Interface version, BCD (`0x0100` = 1.0.0, `0x0120` = 1.2.0).
    pub version: u16,
    pub max_slots: u8,
    pub max_interrupters: u16,
    pub max_ports: u8,
    /// Number of scratchpad buffers (each one page) the controller needs the driver to provide.
    pub scratchpad_buffers: u16,
    /// The controller can use 64-bit addresses (`AC64`).
    pub addresses_64: bool,
    /// Contexts are 64 bytes instead of 32 (`CSZ`).
    pub context_64: bool,
    /// Byte offset of the first extended capability from the MMIO base; `0` if there is none.
    pub ext_caps_offset: u32,
    /// Byte offset of the doorbell array.
    pub doorbell_offset: u32,
    /// Byte offset of the runtime registers.
    pub runtime_offset: u32,
}

impl Capabilities {
    /// Decodes the registers read at [`CAP_LENGTH_VERSION`], [`CAP_HCSPARAMS1`], [`CAP_HCSPARAMS2`],
    /// [`CAP_HCCPARAMS1`], [`CAP_DBOFF`] and [`CAP_RTSOFF`]. `None` when they cannot belong to a working
    /// controller: the window reads all ones (no device behind the BAR), or the length, slot count or port
    /// count is zero.
    pub fn decode(
        length_version: u32,
        hcsparams1: u32,
        hcsparams2: u32,
        hccparams1: u32,
        dboff: u32,
        rtsoff: u32,
    ) -> Option<Self> {
        if length_version == u32::MAX {
            return None;
        }
        let cap_length = length_version as u8;
        let max_slots = hcsparams1 as u8;
        let max_ports = (hcsparams1 >> 24) as u8;
        if cap_length < 0x10 || max_slots == 0 || max_ports == 0 {
            return None;
        }
        let scratch_hi = (hcsparams2 >> 21) & 0x1F;
        let scratch_lo = (hcsparams2 >> 27) & 0x1F;
        Some(Self {
            cap_length,
            version: (length_version >> 16) as u16,
            max_slots,
            max_interrupters: ((hcsparams1 >> 8) & 0x7FF) as u16,
            max_ports,
            scratchpad_buffers: (scratch_hi << 5 | scratch_lo) as u16,
            addresses_64: hccparams1 & 1 != 0,
            context_64: hccparams1 & 0b100 != 0,
            ext_caps_offset: (hccparams1 >> 16) << 2,
            doorbell_offset: dboff & !0b11,
            runtime_offset: rtsoff & !0x1F,
        })
    }

    /// Size in bytes of one context (device, slot or endpoint) for this controller.
    pub const fn context_size(&self) -> usize {
        if self.context_64 { 64 } else { 32 }
    }

    /// Byte offset of the doorbell register of `slot` (slot 0 is the command ring's doorbell).
    pub const fn doorbell(&self, slot: u8) -> u32 {
        self.doorbell_offset + 4 * slot as u32
    }

    /// Byte offset of the register set of interrupter `n` (`IMAN` at `+0x00`; see [`INTR_ERSTSZ`] etc.).
    pub const fn interrupter(&self, n: u16) -> u32 {
        self.runtime_offset + 0x20 + 0x20 * n as u32
    }

    /// Byte offset of `PORTSC` for root-hub port `port` (1-based, as in the specification).
    pub const fn portsc(&self, port: u8) -> u32 {
        self.cap_length as u32 + 0x400 + 0x10 * (port as u32).saturating_sub(1)
    }
}

// ---- operational registers (offsets from base + `cap_length`) ----

pub const OP_USBCMD: u32 = 0x00;
pub const OP_USBSTS: u32 = 0x04;
pub const OP_CRCR: u32 = 0x18;
pub const OP_DCBAAP: u32 = 0x30;
pub const OP_CONFIG: u32 = 0x38;

pub const USBCMD_RUN: u32 = 1 << 0;
pub const USBCMD_HCRST: u32 = 1 << 1;
pub const USBCMD_INTE: u32 = 1 << 2;

pub const USBSTS_HCH: u32 = 1 << 0; // halted
pub const USBSTS_HSE: u32 = 1 << 2; // host system error
pub const USBSTS_EINT: u32 = 1 << 3;
pub const USBSTS_CNR: u32 = 1 << 11; // controller not ready

pub const CRCR_RCS: u64 = 1 << 0; // ring cycle state
pub const CRCR_CRR: u64 = 1 << 3; // command ring running

// ---- interrupter register set (offsets from `Capabilities::interrupter(n)`) ----

pub const INTR_IMAN: u32 = 0x00;
pub const INTR_ERSTSZ: u32 = 0x08;
pub const INTR_ERSTBA: u32 = 0x10;
pub const INTR_ERDP: u32 = 0x18;
/// Event Handler Busy: written as 1 to clear when the dequeue pointer is updated.
pub const ERDP_EHB: u64 = 1 << 3;

/// One Event Ring Segment Table entry: segment base address, size in TRBs, and 32 reserved bits.
pub const fn erst_entry(segment_base: u64, trbs: u16) -> [u32; 4] {
    [
        segment_base as u32,
        (segment_base >> 32) as u32,
        trbs as u32,
        0,
    ]
}

/// Bytes of the Device Context Base Address Array for `max_slots_enabled` slots (entry 0 is the
/// scratchpad array pointer).
pub const fn dcbaa_bytes(max_slots_enabled: u8) -> usize {
    (max_slots_enabled as usize + 1) * 8
}

// ---- ports ----

pub const PORTSC_CCS: u32 = 1 << 0; // current connect status
pub const PORTSC_PED: u32 = 1 << 1; // enabled (RW1C: writing 1 disables the port)
pub const PORTSC_OCA: u32 = 1 << 3;
pub const PORTSC_PR: u32 = 1 << 4; // port reset
pub const PORTSC_PP: u32 = 1 << 9; // port power
pub const PORTSC_CAS: u32 = 1 << 24;
pub const PORTSC_DR: u32 = 1 << 30;
pub const PORTSC_CSC: u32 = 1 << 17;
pub const PORTSC_PEC: u32 = 1 << 18;
pub const PORTSC_WRC: u32 = 1 << 19;
pub const PORTSC_OCC: u32 = 1 << 20;
pub const PORTSC_PRC: u32 = 1 << 21; // port reset change
pub const PORTSC_PLC: u32 = 1 << 22;
pub const PORTSC_CEC: u32 = 1 << 23;

/// Every "change" bit of `PORTSC`: all are write-1-to-clear.
pub const PORTSC_CHANGES: u32 =
    PORTSC_CSC | PORTSC_PEC | PORTSC_WRC | PORTSC_OCC | PORTSC_PRC | PORTSC_PLC | PORTSC_CEC;

/// Bits that read back and must be written back unchanged (read-only ones are ignored on write, the
/// read/write ones keep their value): connect status, over-current, speed, CAS, DR, indicator, link
/// state, power, wake enables.
const PORTSC_PRESERVE: u32 = PORTSC_CCS
    | PORTSC_OCA
    | (0xF << 10) // port speed
    | (0x3 << 14) // port indicator
    | (0xF << 5) // port link state
    | PORTSC_PP
    | PORTSC_CAS
    | (0x7 << 25) // wake on connect / disconnect / over-current
    | PORTSC_DR;

/// The value to write to `PORTSC` to change nothing: the register read with the write-1-to-clear bits
/// (`PED` and the change bits) dropped, so a read-modify-write cannot disable the port or lose a change
/// the driver has not looked at.
pub const fn portsc_neutral(read: u32) -> u32 {
    read & PORTSC_PRESERVE
}

/// Value that starts a port reset (`PR`) without disturbing anything else.
pub const fn portsc_reset(read: u32) -> u32 {
    portsc_neutral(read) | PORTSC_PR
}

/// Value that acknowledges the change bits in `which` (write 1 to clear) and nothing else.
pub const fn portsc_ack(read: u32, which: u32) -> u32 {
    portsc_neutral(read) | (which & PORTSC_CHANGES)
}

/// Speed of a device, from the Port Speed ID in `PORTSC` bits 13:10 (the default protocol speed IDs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speed {
    Low,
    Full,
    High,
    Super,
}

impl Speed {
    pub const fn from_portsc(portsc: u32) -> Option<Self> {
        match (portsc >> 10) & 0xF {
            1 => Some(Speed::Full),
            2 => Some(Speed::Low),
            3 => Some(Speed::High),
            4 => Some(Speed::Super),
            _ => None,
        }
    }

    /// The value of the Speed field in a slot context.
    pub const fn slot_id(self) -> u32 {
        match self {
            Speed::Full => 1,
            Speed::Low => 2,
            Speed::High => 3,
            Speed::Super => 4,
        }
    }

    /// Packet size of the control endpoint to start with, before the first 8 bytes of the device
    /// descriptor are read. Full speed starts at 8, which every full-speed device accepts.
    pub const fn initial_ep0_max_packet(self) -> u16 {
        match self {
            Speed::Low | Speed::Full => 8,
            Speed::High => 64,
            Speed::Super => 512,
        }
    }

    /// The control endpoint's real packet size from `bMaxPacketSize0` in the device descriptor. A
    /// SuperSpeed device reports an exponent (`9` = 512); the others report the size. `None` for a value
    /// the speed does not allow.
    pub const fn ep0_max_packet(self, b_max_packet_size0: u8) -> Option<u16> {
        match (self, b_max_packet_size0) {
            (Speed::Super, 9) => Some(512),
            (Speed::High, 64) => Some(64),
            (Speed::Full, 8 | 16 | 32 | 64) => Some(b_max_packet_size0 as u16),
            (Speed::Low, 8) => Some(8),
            _ => None,
        }
    }
}

// ---- extended capabilities ----

pub const EXT_CAP_LEGACY_SUPPORT: u8 = 1;
pub const EXT_CAP_SUPPORTED_PROTOCOL: u8 = 2;

/// Upper bound on the list length; a list that never ends (hostile or broken hardware) stops here.
const EXT_CAP_MAX: usize = 64;

/// Byte offset (from the MMIO base) of the first extended capability with id `id`. `first` is
/// [`Capabilities::ext_caps_offset`]; `read32(offset)` reads a dword of the window.
pub fn find_ext_cap(first: u32, id: u8, mut read32: impl FnMut(u32) -> u32) -> Option<u32> {
    let mut offset = first;
    if offset == 0 {
        return None;
    }
    for _ in 0..EXT_CAP_MAX {
        let header = read32(offset);
        if header == u32::MAX {
            return None;
        }
        if header as u8 == id {
            return Some(offset);
        }
        let next = (header >> 8) as u8;
        if next == 0 {
            return None;
        }
        offset = offset.checked_add(u32::from(next) * 4)?;
    }
    None
}

const LEGACY_BIOS_OWNED: u32 = 1 << 16;
const LEGACY_OS_OWNED: u32 = 1 << 24;

/// `USBLEGSUP` value that asks the firmware to hand the controller over (sets the OS-owned semaphore).
pub const fn legacy_request_os_ownership(usblegsup: u32) -> u32 {
    usblegsup | LEGACY_OS_OWNED
}

/// The firmware has let go: its semaphore is clear and ours is set.
pub const fn legacy_handoff_done(usblegsup: u32) -> bool {
    usblegsup & LEGACY_BIOS_OWNED == 0 && usblegsup & LEGACY_OS_OWNED != 0
}
