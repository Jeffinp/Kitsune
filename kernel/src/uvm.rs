//! Physical frames and user address spaces (ring 3).
//!
//! A user process has its own page-table root. The kernel half of it is a copy of the kernel's
//! top-level entries, so kernel code, stacks, the heap and the physical-memory map stay mapped
//! while a user thread runs a system call or takes an interrupt; those entries are supervisor-only
//! (no `USER` bit), so ring 3 cannot touch any of it. The user half is top-level entry 0
//! ([`USER_BASE`]..[`USER_END`], the low 512 GiB). The bootloader left a few identity mappings there
//! that only the hand-over to the kernel needed; a user address space does not inherit them (its entry
//! 0 starts empty), so user code and the kernel's code both sit in the low 2 GiB model-wise: programs
//! link at the usual 4 MiB with the small code model.
//!
//! Frames come from [`FrameAlloc`](kitsune_core::paging::FrameAlloc), built from the bootloader's
//! usable memory regions. Every frame is zeroed before it is handed out. Page tables are edited
//! through the bootloader's physical-memory map, never through the live mapping, so the code does
//! not depend on which address space is active.
//!
//! User memory is always reached by translating addresses page by page ([`AddressSpace::copy_in`]
//! / [`AddressSpace::copy_out`]); the kernel never dereferences a user pointer directly.

use crate::allocator::SpinLock;
use kitsune_core::paging::{FrameAlloc, PAGE_SIZE};

/// First user virtual address (the first 4 MiB stay unmapped, so a null pointer faults).
pub const USER_BASE: u64 = 0x0000_0000_0040_0000;
/// One past the last user virtual address (the end of top-level entry 0).
pub const USER_END: u64 = 0x0000_0080_0000_0000;
/// Top-level index of the user half.
const USER_PML4: usize = 0;

const PRESENT: u64 = 1 << 0;
const WRITABLE: u64 = 1 << 1;
const USER: u64 = 1 << 2;
const NX: u64 = 1 << 63;
const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

static FRAMES: SpinLock<Option<FrameAlloc>> = SpinLock::new(None);

/// Give the frame allocator the usable physical memory (bytes `[start, end)` ranges).
pub fn init_frames(regions: &[(u64, u64)]) {
    let a = FrameAlloc::new(regions);
    crate::klog!(Info, "frames: {} usable ({} KiB)", a.total(), a.total() * 4);
    *FRAMES.lock() = Some(a);
}

/// `(used, total)` frames of the allocator.
pub fn frame_stats() -> (usize, usize) {
    FRAMES
        .lock()
        .as_ref()
        .map_or((0, 0), |a| (a.used(), a.total()))
}

fn table_ptr(phys: u64) -> *mut u64 {
    (crate::vm::phys_offset() + phys) as *mut u64
}

/// A zeroed frame, or `None` when memory is exhausted.
fn alloc_frame() -> Option<u64> {
    let f = FRAMES.lock().as_mut()?.alloc()?;
    // SAFETY: `f` is a whole frame the allocator just handed out (nobody else has it), reached through
    // the physical map, so zeroing 4096 bytes there stays inside it.
    unsafe { core::ptr::write_bytes(table_ptr(f) as *mut u8, 0, PAGE_SIZE as usize) };
    Some(f)
}

fn free_frame(f: u64) {
    if let Some(a) = FRAMES.lock().as_mut()
        && a.free(f).is_err()
    {
        crate::klog!(Error, "frames: bad free of {f:#x}");
    }
}

/// Why a mapping or a user access was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UvmError {
    /// No frame left.
    OutOfMemory,
    /// The address is outside the user range, or not page aligned where it must be.
    BadAddress,
    /// That page is already mapped.
    Mapped,
    /// A page the access needs is not mapped (or not writable for a write).
    Fault,
}

/// One user address space: the physical address of its top-level table.
pub struct AddressSpace {
    pml4: u64,
}

impl AddressSpace {
    /// A fresh address space with the kernel's mappings and an empty user half.
    pub fn new() -> Result<AddressSpace, UvmError> {
        let kernel = x86_64::registers::control::Cr3::read()
            .0
            .start_address()
            .as_u64();
        let pml4 = alloc_frame().ok_or(UvmError::OutOfMemory)?;
        for i in 0..512usize {
            if i == USER_PML4 {
                continue;
            }
            // SAFETY: both are 4 KiB tables reached through the physical map and `i < 512`; the new one
            // is private to us, the kernel's is only read.
            unsafe {
                table_ptr(pml4)
                    .add(i)
                    .write(table_ptr(kernel).add(i).read())
            };
        }
        Ok(AddressSpace { pml4 })
    }

    /// Write `src` at user address `va` through the kernel's own view, even into pages the user maps
    /// read-only (the loader filling code and constants). The pages must be mapped.
    pub fn poke(&self, va: u64, src: &[u8]) -> Result<(), UvmError> {
        let mut done = 0usize;
        while done < src.len() {
            let a = va.checked_add(done as u64).ok_or(UvmError::Fault)?;
            let (frame, _) = self.walk(a & !(PAGE_SIZE - 1)).ok_or(UvmError::Fault)?;
            let off = (a % PAGE_SIZE) as usize;
            let n = (PAGE_SIZE as usize - off).min(src.len() - done);
            // SAFETY: as in `copy_out`: a mapped user frame, `off + n` inside the page.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    src[done..].as_ptr(),
                    (table_ptr(frame) as *mut u8).add(off),
                    n,
                );
            }
            done += n;
        }
        Ok(())
    }

    /// The value to load into CR3 (physical address of the top-level table).
    pub fn root(&self) -> u64 {
        self.pml4
    }

    /// Next table down from `entry_ptr`, creating it (zeroed, user-accessible) when absent.
    fn descend(entry_ptr: *mut u64) -> Result<u64, UvmError> {
        // SAFETY: `entry_ptr` points into a live table of this address space (see `walk`).
        let e = unsafe { entry_ptr.read() };
        if e & PRESENT != 0 {
            return Ok(e & ADDR_MASK);
        }
        let t = alloc_frame().ok_or(UvmError::OutOfMemory)?;
        // SAFETY: as above; the new entry points at a zeroed frame we own.
        unsafe { entry_ptr.write(t | PRESENT | WRITABLE | USER) };
        Ok(t)
    }

    /// Map the user page at `va` to `frame`. The frame becomes this address space's (freed with it).
    pub fn map(&mut self, va: u64, frame: u64, writable: bool, exec: bool) -> Result<(), UvmError> {
        if !va.is_multiple_of(PAGE_SIZE) || !(USER_BASE..USER_END).contains(&va) {
            return Err(UvmError::BadAddress);
        }
        let idx = |shift: u32| ((va >> shift) & 0x1FF) as usize;
        let mut table = self.pml4;
        for shift in [39u32, 30, 21] {
            // SAFETY: `table` is one of our live tables, `idx < 512`.
            let ptr = unsafe { table_ptr(table).add(idx(shift)) };
            table = Self::descend(ptr)?;
        }
        // SAFETY: `table` is our level-1 table, `idx < 512`.
        let leaf = unsafe { table_ptr(table).add(idx(12)) };
        // SAFETY: as above.
        if unsafe { leaf.read() } & PRESENT != 0 {
            return Err(UvmError::Mapped);
        }
        let mut flags = PRESENT | USER;
        if writable {
            flags |= WRITABLE;
        }
        if !exec {
            flags |= NX;
        }
        // SAFETY: as above.
        unsafe { leaf.write(frame | flags) };
        Ok(())
    }

    /// Allocate and map a zeroed page at `va`.
    pub fn map_new(&mut self, va: u64, writable: bool, exec: bool) -> Result<u64, UvmError> {
        let f = alloc_frame().ok_or(UvmError::OutOfMemory)?;
        match self.map(va, f, writable, exec) {
            Ok(()) => Ok(f),
            Err(e) => {
                free_frame(f);
                Err(e)
            }
        }
    }

    /// Physical address and entry flags of the page holding `va`, if it is mapped for the user.
    fn walk(&self, va: u64) -> Option<(u64, u64)> {
        if !(USER_BASE..USER_END).contains(&va) {
            return None;
        }
        let mut table = self.pml4;
        for shift in [39u32, 30, 21, 12] {
            let i = ((va >> shift) & 0x1FF) as usize;
            // SAFETY: `table` is one of our live tables, `i < 512`; read only.
            let e = unsafe { table_ptr(table).add(i).read() };
            if e & PRESENT == 0 || e & USER == 0 {
                return None;
            }
            if shift == 12 {
                return Some((e & ADDR_MASK, e));
            }
            table = e & ADDR_MASK;
        }
        None
    }

    /// Copy `dst.len()` bytes from user address `va` into `dst`.
    pub fn copy_in(&self, va: u64, dst: &mut [u8]) -> Result<(), UvmError> {
        let mut done = 0usize;
        while done < dst.len() {
            let a = va.checked_add(done as u64).ok_or(UvmError::Fault)?;
            let (frame, _) = self.walk(a & !(PAGE_SIZE - 1)).ok_or(UvmError::Fault)?;
            let off = (a % PAGE_SIZE) as usize;
            let n = (PAGE_SIZE as usize - off).min(dst.len() - done);
            // SAFETY: `frame` is a mapped user frame reached through the physical map; `off + n` stays
            // inside that 4 KiB page; `dst[done..done + n]` is a distinct kernel buffer.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (table_ptr(frame) as *const u8).add(off),
                    dst[done..].as_mut_ptr(),
                    n,
                );
            }
            done += n;
        }
        Ok(())
    }

    /// Free every user frame, every table below the root and the root itself. Must not be the live
    /// address space.
    pub fn destroy(self) {
        let l4 = entry(self.pml4, USER_PML4);
        if l4 & PRESENT != 0 {
            for i3 in 0..512 {
                let e3 = entry(l4 & ADDR_MASK, i3);
                if e3 & PRESENT == 0 {
                    continue;
                }
                for i2 in 0..512 {
                    let e2 = entry(e3 & ADDR_MASK, i2);
                    if e2 & PRESENT == 0 {
                        continue;
                    }
                    for i1 in 0..512 {
                        let e1 = entry(e2 & ADDR_MASK, i1);
                        if e1 & PRESENT != 0 {
                            free_frame(e1 & ADDR_MASK);
                        }
                    }
                    free_frame(e2 & ADDR_MASK);
                }
                free_frame(e3 & ADDR_MASK);
            }
            free_frame(l4 & ADDR_MASK);
        }
        free_frame(self.pml4);
    }
}

/// Entry `i` of the page table in physical frame `table`.
fn entry(table: u64, i: usize) -> u64 {
    assert!(i < 512);
    // SAFETY: `table` is a frame holding one of an address space's own tables (callers walk down from
    // its root through present entries), reached through the physical map; `i < 512` keeps the read
    // inside that 4 KiB table.
    unsafe { table_ptr(table).add(i).read() }
}
