//! Minimal PCI configuration-space access (legacy mechanism #1) and bus
//! enumeration — the foundation for the virtio and USB drivers. It locates devices
//! by reading config space through the 0xCF8/0xCFC port pair.

use crate::io::{inl, outl};

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

use kitsune_core::hw::pci::{
    VIRTIO_GPU_LEGACY, VIRTIO_GPU_MODERN, VIRTIO_NET_LEGACY, VIRTIO_NET_MODERN, VIRTIO_RNG_LEGACY,
    VIRTIO_RNG_MODERN, VIRTIO_VENDOR, bar_is_64bit, bar_offset, bar_size, cap_list_start,
    config_address, extract_u16,
};

/// A located PCI function.
#[derive(Clone, Copy, Debug)]
pub struct PciDevice {
    pub bus: u8,
    pub slot: u8,
    pub func: u8,
    pub vendor: u16,
    pub device: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
}

/// Read a 32-bit dword from config space (`offset` is dword-aligned).
pub fn read32(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    outl(CONFIG_ADDRESS, config_address(bus, slot, func, offset));
    inl(CONFIG_DATA)
}

/// Read a 16-bit word from config space.
pub fn read16(bus: u8, slot: u8, func: u8, offset: u8) -> u16 {
    extract_u16(read32(bus, slot, func, offset & 0xFC), offset)
}

/// Write a 32-bit dword to config space.
pub fn write32(bus: u8, slot: u8, func: u8, offset: u8, value: u32) {
    outl(CONFIG_ADDRESS, config_address(bus, slot, func, offset));
    outl(CONFIG_DATA, value);
}

impl PciDevice {
    /// Raw value of base address register `i` (0..6); 0 for a non-existent BAR.
    pub fn bar(&self, i: u8) -> u32 {
        match bar_offset(i) {
            Some(off) => read32(self.bus, self.slot, self.func, off),
            None => 0,
        }
    }

    /// Set the memory-space + bus-master bits in the command register, required
    /// before a DMA-capable device (like virtio-gpu) can be used.
    pub fn enable_bus_master(&self) {
        let mut cmd = read32(self.bus, self.slot, self.func, 0x04);
        cmd |= 0x0006; // bit1 = memory space, bit2 = bus master
        write32(self.bus, self.slot, self.func, 0x04, cmd);
    }

    /// Size in bytes of memory BAR `i`, found the standard way: decoding off, write all ones, read back
    /// what stuck, restore. `None` for an I/O or unimplemented BAR. Done once, before the driver
    /// trusts any offset the device reports.
    pub fn probe_bar_size(&self, i: u8) -> Option<u64> {
        let off = bar_offset(i)?;
        let command = read32(self.bus, self.slot, self.func, 0x04);
        write32(self.bus, self.slot, self.func, 0x04, command & !0x7);
        let lo = read32(self.bus, self.slot, self.func, off);
        let is_64 = bar_is_64bit(lo);
        let hi = if is_64 {
            bar_offset(i + 1).map(|o| (o, read32(self.bus, self.slot, self.func, o)))
        } else {
            None
        };
        write32(self.bus, self.slot, self.func, off, u32::MAX);
        let probed_lo = read32(self.bus, self.slot, self.func, off);
        let mut probed_hi = 0;
        if let Some((o, _)) = hi {
            write32(self.bus, self.slot, self.func, o, u32::MAX);
            probed_hi = read32(self.bus, self.slot, self.func, o);
        }
        write32(self.bus, self.slot, self.func, off, lo);
        if let Some((o, original)) = hi {
            write32(self.bus, self.slot, self.func, o, original);
        }
        write32(self.bus, self.slot, self.func, 0x04, command);
        if is_64 && hi.is_none() {
            return None;
        }
        bar_size(probed_lo, probed_hi)
    }

    /// Offset of the first entry in the PCI capability list, or `None` if the
    /// device has none (status register bit 4 clears it).
    pub fn cap_list(&self) -> Option<u8> {
        let status = read16(self.bus, self.slot, self.func, 0x06);
        let ptr = read16(self.bus, self.slot, self.func, 0x34);
        cap_list_start(status, ptr)
    }

    /// Read a config-space dword at `offset` (for walking capabilities).
    pub fn cap_read32(&self, offset: u8) -> u32 {
        read32(self.bus, self.slot, self.func, offset)
    }
}

/// Visit every present function on every bus reachable from bus 0 (bridges are followed), calling
/// `f` for each. Devices behind PCI-to-PCI bridges (the usual place of an xHCI on real hardware) are
/// found this way; QEMU's default machine keeps everything on bus 0.
pub fn for_each<F: FnMut(PciDevice)>(mut f: F) {
    kitsune_core::hw::pci::enumerate_all(read32, |d| {
        f(PciDevice {
            bus: d.bus,
            slot: d.slot,
            func: d.func,
            vendor: d.vendor,
            device: d.device,
            class: d.class,
            subclass: d.subclass,
            prog_if: d.prog_if,
        })
    });
}

/// Find the first USB xHCI host controller.
pub fn find_xhci() -> Option<PciDevice> {
    let mut found = None;
    for_each(|d| {
        if found.is_none()
            && d.class == kitsune_core::hw::pci::CLASS_SERIAL_BUS
            && d.subclass == kitsune_core::hw::pci::SUBCLASS_USB
            && d.prog_if == kitsune_core::hw::pci::PROG_IF_XHCI
        {
            found = Some(d);
        }
    });
    found
}

/// Find the first function matching `vendor`/`device` on bus 0.
pub fn find(vendor: u16, device: u16) -> Option<PciDevice> {
    let mut found = None;
    for_each(|d| {
        if found.is_none() && d.vendor == vendor && d.device == device {
            found = Some(d);
        }
    });
    found
}

/// Locate a virtio-gpu device (modern, then transitional).
pub fn find_virtio_gpu() -> Option<PciDevice> {
    find(VIRTIO_VENDOR, VIRTIO_GPU_MODERN).or_else(|| find(VIRTIO_VENDOR, VIRTIO_GPU_LEGACY))
}

/// Locate a virtio-net device (modern, then transitional).
pub fn find_virtio_net() -> Option<PciDevice> {
    find(VIRTIO_VENDOR, VIRTIO_NET_MODERN).or_else(|| find(VIRTIO_VENDOR, VIRTIO_NET_LEGACY))
}

/// Locate a virtio-rng (entropy) device (modern, then transitional).
pub fn find_virtio_rng() -> Option<PciDevice> {
    find(VIRTIO_VENDOR, VIRTIO_RNG_MODERN).or_else(|| find(VIRTIO_VENDOR, VIRTIO_RNG_LEGACY))
}
