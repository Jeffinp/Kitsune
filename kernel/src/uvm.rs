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

/// Arm SMEP and SMAP when the CPU has them: the kernel then faults if it ever executes (SMEP) or
/// reads and writes (SMAP) a page marked user, whatever a bug or a hostile process arranged. The
/// kernel never dereferences user pointers (every access goes through the translated copies below),
/// so SMAP needs no `stac`/`clac` windows. Returns which of the two are on.
pub fn harden() -> (bool, bool) {
    use x86_64::registers::control::{Cr4, Cr4Flags};
    // CPUID leaf 7, subleaf 0: EBX bit 7 = SMEP, bit 20 = SMAP.
    let leaf = core::arch::x86_64::__cpuid_count(7, 0);
    let (smep, smap) = (leaf.ebx & (1 << 7) != 0, leaf.ebx & (1 << 20) != 0);
    let mut flags = Cr4::read();
    if smep {
        flags |= Cr4Flags::SUPERVISOR_MODE_EXECUTION_PROTECTION;
    }
    if smap {
        flags |= Cr4Flags::SUPERVISOR_MODE_ACCESS_PREVENTION;
    }
    // SAFETY: only SMEP/SMAP bits are added to CR4, each only when CPUID says the CPU implements it,
    // and the kernel neither runs user code nor touches user pages directly, so neither can fault.
    unsafe { Cr4::write(flags) };
    crate::klog!(
        Info,
        "cpu: SMEP {}, SMAP {}",
        if smep { "on" } else { "unavailable" },
        if smap { "on" } else { "unavailable" }
    );
    (smep, smap)
}

/// Let ring 3 code use SSE (x86_64 always has SSE2): CR4.OSFXSR says the OS saves the SSE state with
/// `fxsave`/`fxrstor` (the scheduler does, per thread, see `sched::FxArea`), CR4.OSXMMEXCPT routes
/// unmasked SIMD floating-point exceptions to `#XM` (which kills only the offending process), and
/// CR0.EM off / CR0.MP on make the instructions run instead of raising `#UD`/`#NM`. Without this a
/// program compiled with SSE (the C of `jsrun`: QuickJS uses hardware doubles) dies on its first
/// `movups`. The kernel itself stays soft-float. Must run before the first thread's save area is
/// seeded (`sched::init`), so every area starts with a valid MXCSR.
pub fn enable_sse() {
    use x86_64::registers::control::{Cr0, Cr0Flags, Cr4, Cr4Flags};
    let mut cr0 = Cr0::read();
    cr0.remove(Cr0Flags::EMULATE_COPROCESSOR | Cr0Flags::TASK_SWITCHED);
    cr0.insert(Cr0Flags::MONITOR_COPROCESSOR);
    // SAFETY: only the coprocessor bits change; every x86_64 CPU has an FPU and SSE2.
    unsafe { Cr0::write(cr0) };
    let cr4 = Cr4::read() | Cr4Flags::OSFXSR | Cr4Flags::OSXMMEXCPT_ENABLE;
    // SAFETY: both bits only enable SSE state handling the scheduler already performs (`fxsave` /
    // `fxrstor` on every switch) and the `#XM` exception, which has a handler.
    unsafe { Cr4::write(cr4) };
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
