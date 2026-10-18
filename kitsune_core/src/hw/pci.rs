//! PCI configuration-space decoding: addresses, BARs, capability-list start and
//! bus enumeration. The kernel supplies the port I/O; this module supplies the
//! arithmetic.

/// virtio PCI vendor id, and the modern/transitional virtio-gpu device ids.
pub const VIRTIO_VENDOR: u16 = 0x1AF4;
pub const VIRTIO_GPU_MODERN: u16 = 0x1050; // 0x1040 + virtio device type 16 (GPU)
pub const VIRTIO_GPU_LEGACY: u16 = 0x1010;

/// virtio-net device ids: transitional (also what QEMU's default `virtio-net-pci`
/// reports) and modern (0x1040 + device type 1).
pub const VIRTIO_NET_LEGACY: u16 = 0x1000;
pub const VIRTIO_NET_MODERN: u16 = 0x1041;

/// virtio-rng (entropy source) device ids: transitional (QEMU's default `virtio-rng-pci`)
/// and modern (0x1040 + device type 4).
pub const VIRTIO_RNG_LEGACY: u16 = 0x1005;
pub const VIRTIO_RNG_MODERN: u16 = 0x1044;

/// `true` for a modern or transitional virtio-rng `vendor:device` pair.
pub fn is_virtio_rng(vendor: u16, device: u16) -> bool {
    vendor == VIRTIO_VENDOR && (device == VIRTIO_RNG_MODERN || device == VIRTIO_RNG_LEGACY)
}

/// `true` for a modern or transitional virtio-net `vendor:device` pair.
pub fn is_virtio_net(vendor: u16, device: u16) -> bool {
    vendor == VIRTIO_VENDOR && (device == VIRTIO_NET_MODERN || device == VIRTIO_NET_LEGACY)
}

/// `true` for a modern or transitional virtio-gpu `vendor:device` pair.
pub fn is_virtio_gpu(vendor: u16, device: u16) -> bool {
    vendor == VIRTIO_VENDOR && (device == VIRTIO_GPU_MODERN || device == VIRTIO_GPU_LEGACY)
}

/// Value for the `0xCF8` CONFIG_ADDRESS port selecting a dword.
pub fn config_address(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    0x8000_0000
        | (bus as u32) << 16
        | (slot as u32) << 11
        | (func as u32) << 8
        | (offset as u32 & 0xFC)
}

/// The 16-bit word at byte `offset` out of the dword read at `offset & 0xFC`.
pub fn extract_u16(dword: u32, offset: u8) -> u16 {
    ((dword >> ((offset as u32 & 2) * 8)) & 0xFFFF) as u16
}

/// Status register bit 4: a capability list exists.
const STATUS_CAP_LIST: u16 = 0x10;

/// Offset of the first PCI capability, given the status register (offset 0x06)
/// and the capability-pointer word (offset 0x34). `None` when the device has no
/// list. The pointer's low two bits are reserved and masked off.
pub fn cap_list_start(status: u16, cap_ptr_word: u16) -> Option<u8> {
    if status & STATUS_CAP_LIST == 0 {
        return None;
    }
    Some((cap_ptr_word & 0xFC) as u8)
}

/// `true` when a BAR's low dword describes a 64-bit memory BAR (type bits
/// `0b10` in bits 2:1), whose high half lives in the next BAR slot.
pub fn bar_is_64bit(lo: u32) -> bool {
    lo & 0b110 == 0b100
}

/// Physical base address of a BAR from its low dword and (for 64-bit BARs) the
/// following dword; the four low flag bits are masked. `hi` is ignored for
/// 32-bit BARs.
pub fn bar_address(lo: u32, hi: u32) -> u64 {
    if bar_is_64bit(lo) {
        ((hi as u64) << 32) | (lo as u64 & !0xF)
    } else {
        lo as u64 & !0xF
    }
}

/// Number of base address registers in a type-0 header.
pub const BAR_COUNT: u8 = 6;

/// Config-space offset of BAR `index`, or `None` if it is not one of the six.
pub fn bar_offset(index: u8) -> Option<u8> {
    (index < BAR_COUNT).then(|| 0x10 + index * 4)
}

/// Memory base address of BAR `index` from its low dword and the dword of the
/// next slot (`hi`, only used by 64-bit BARs). `None` when the BAR cannot hold
/// an MMIO window: bad index, an I/O-space BAR, a 64-bit BAR in the last slot
/// (no room for its high half), or an unassigned (zero) base.
pub fn memory_bar_base(index: u8, lo: u32, hi: u32) -> Option<u64> {
    if index >= BAR_COUNT || lo & 1 != 0 {
        return None;
    }
    if bar_is_64bit(lo) && index + 1 >= BAR_COUNT {
        return None;
    }
    let base = bar_address(lo, hi);
    (base != 0).then_some(base)
}

/// Walks bus 0: calls `f(slot, func, vendor, device)` for every present
/// function. `read16(slot, func, offset)` reads config space. A slot whose
/// function 0 is absent is skipped entirely; other absent functions are skipped
/// individually.
pub fn enumerate(mut read16: impl FnMut(u8, u8, u8) -> u16, mut f: impl FnMut(u8, u8, u16, u16)) {
    for slot in 0..32u8 {
        for func in 0..8u8 {
            let vendor = read16(slot, func, 0x00);
            if vendor == 0xFFFF {
                if func == 0 {
                    break; // no function 0 -> the whole slot is empty
                }
                continue;
            }
            let device = read16(slot, func, 0x02);
            f(slot, func, vendor, device);
        }
    }
}

/// Class code of USB host controllers: base class 0x0C (serial bus), subclass 0x03 (USB).
pub const CLASS_SERIAL_BUS: u8 = 0x0C;
pub const SUBCLASS_USB: u8 = 0x03;
/// Programming interface of an xHCI controller (EHCI is 0x20, OHCI 0x10, UHCI 0x00).
pub const PROG_IF_XHCI: u8 = 0x30;

/// A present PCI function found by [`enumerate_all`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Function {
    pub bus: u8,
    pub slot: u8,
    pub func: u8,
    pub vendor: u16,
    pub device: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    /// Header type byte (offset 0x0E), bit 7 (multi-function) included.
    pub header_type: u8,
}

impl Function {
    /// PCI-to-PCI bridge (header type 1): another bus hangs behind it.
    pub fn is_bridge(&self) -> bool {
        self.header_type & 0x7F == 1
    }

    /// USB 3.x host controller (class 0x0C, subclass 0x03, programming interface 0x30).
    pub fn is_xhci(&self) -> bool {
        self.class == CLASS_SERIAL_BUS
            && self.subclass == SUBCLASS_USB
            && self.prog_if == PROG_IF_XHCI
    }
}

/// Walks every bus reachable from bus 0 by following PCI-to-PCI bridges, calling `f` for each present
/// function in discovery order. `read32(bus, slot, func, offset)` reads a config-space dword.
///
/// Real machines put most devices (xHCI, NICs, disks) behind bridges, so a scan of bus 0 alone misses
/// them. The config space is untrusted: a bridge whose secondary bus is not greater than its own bus
/// (self-loop, backwards link) is not followed, and each bus is scanned at most once, so any content
/// terminates. Functions 1..7 of a device are read only when its header type says it is multi-function.
pub fn enumerate_all(mut read32: impl FnMut(u8, u8, u8, u8) -> u32, mut f: impl FnMut(Function)) {
    let mut seen = [false; 256];
    let mut pending = [0u8; 256];
    let mut len = 1;
    seen[0] = true; // pending[0] is bus 0
    while len > 0 {
        len -= 1;
        let bus = pending[len];
        for slot in 0..32u8 {
            let mut functions = 8;
            let mut func = 0;
            while func < functions {
                let id = read32(bus, slot, func, 0x00);
                if id & 0xFFFF == 0xFFFF {
                    if func == 0 {
                        break; // no function 0 -> the whole slot is empty
                    }
                    func += 1;
                    continue;
                }
                let class = read32(bus, slot, func, 0x08);
                let header_type = (read32(bus, slot, func, 0x0C) >> 16) as u8;
                if func == 0 && header_type & 0x80 == 0 {
                    functions = 1;
                }
                let d = Function {
                    bus,
                    slot,
                    func,
                    vendor: id as u16,
                    device: (id >> 16) as u16,
                    class: (class >> 24) as u8,
                    subclass: (class >> 16) as u8,
                    prog_if: (class >> 8) as u8,
                    header_type,
                };
                f(d);
                if d.is_bridge() {
                    let secondary = (read32(bus, slot, func, 0x18) >> 8) as u8;
                    if secondary > bus && !seen[secondary as usize] {
                        seen[secondary as usize] = true;
                        pending[len] = secondary;
                        len += 1;
                    }
                }
                func += 1;
            }
        }
    }
}

/// Size in bytes of a memory BAR from what it reads back after writing all ones to it
/// (`probed_lo`, and for a 64-bit BAR `probed_hi`, the next slot probed the same way). `None` for an
/// I/O-space BAR, an unimplemented one (no writable address bit), or a size that does not fit in `u64`.
/// The caller restores the original BAR value and keeps decoding off while probing.
pub fn bar_size(probed_lo: u32, probed_hi: u32) -> Option<u64> {
    if probed_lo & 1 != 0 {
        return None;
    }
    if bar_is_64bit(probed_lo) {
        let mask = (probed_hi as u64) << 32 | (probed_lo as u64 & !0xF);
        (mask != 0)
            .then(|| (!mask).wrapping_add(1))
            .filter(|&s| s != 0)
    } else {
        let mask = probed_lo & !0xF;
        (mask != 0).then(|| (!mask).wrapping_add(1) as u64)
    }
}

#[cfg(test)]
mod tests;
