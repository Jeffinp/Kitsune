//! Volatile access to the controller's register window, bounds-checked against the BAR size read
//! from PCI. Every offset in the driver comes from the specification or from the device itself
//! (doorbell array, runtime registers, ports), so a confused device must not make the kernel read
//! whatever physical memory happens to follow its window.

#[derive(Clone, Copy)]
pub struct Mmio {
    base: u64,
    len: u32,
}

impl Mmio {
    /// `base` is the virtual address (through the physical-memory map) of the start of the window and
    /// `len` its size in bytes; the caller has checked that the whole range is mapped.
    pub fn new(base: u64, len: u32) -> Self {
        Self { base, len }
    }

    /// `true` if a dword at `off` lies inside the window.
    pub fn contains(&self, off: u32) -> bool {
        off & 3 == 0 && off.checked_add(4).is_some_and(|end| end <= self.len)
    }

    /// Reads a dword; all ones (what an empty bus returns) if `off` is outside the window.
    pub fn r32(&self, off: u32) -> u32 {
        if !self.contains(off) {
            return u32::MAX;
        }
        // SAFETY: `off` is 4-aligned and `off + 4 <= len`, and `new`'s caller guaranteed that
        // `base..base + len` is mapped device memory, so this is an aligned in-window MMIO read.
        unsafe { core::ptr::read_volatile((self.base + u64::from(off)) as *const u32) }
    }

    /// Writes a dword; ignored if `off` is outside the window.
    pub fn w32(&self, off: u32, value: u32) {
        if !self.contains(off) {
            return;
        }
        // SAFETY: as in `r32`: aligned, inside the mapped window.
        unsafe { core::ptr::write_volatile((self.base + u64::from(off)) as *mut u32, value) }
    }

    /// Writes a 64-bit register as two dwords, low first (what the specification asks of drivers
    /// that do not use 64-bit accesses).
    pub fn w64(&self, off: u32, value: u64) {
        self.w32(off, value as u32);
        self.w32(off + 4, (value >> 32) as u32);
    }
}
