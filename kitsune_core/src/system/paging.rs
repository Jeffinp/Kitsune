//! Pure x86_64 paging arithmetic and thread-stack layout with a guard page.
//!
//! The kernel walks the bootloader's page tables (through the physical-memory
//! mapping) to take one page out of a thread stack's block: a *guard page* below
//! the stack, whose `PRESENT` bit is cleared so a stack overflow faults instead of
//! silently writing into whatever the heap put next to it. The `unsafe` walk lives
//! in `kernel/src/vm.rs`; the decisions it makes (which table index at which
//! level, whether an entry may be descended into, how a block splits into guard
//! and stack, whether a faulting address hit a guard) are here, where they are
//! tested on the host.

mod frames;

pub use frames::{FrameAlloc, FrameError};

/// Size of a (small) page.
pub const PAGE_SIZE: u64 = 4096;
/// Entries per page table.
pub const ENTRIES: usize = 512;

/// Page-table entry bit 0: the entry maps something.
pub const ENTRY_PRESENT: u64 = 1 << 0;
/// Page-table entry bit 7 (levels 2 and 3): the entry maps a 2 MiB / 1 GiB page
/// directly instead of pointing to a lower table.
pub const ENTRY_HUGE: u64 = 1 << 7;
/// Bits 12..51 of an entry: the physical address of the next table / the frame.
pub const ENTRY_ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;

/// Round `addr` down to a page boundary.
pub const fn page_floor(addr: u64) -> u64 {
    addr & !(PAGE_SIZE - 1)
}

/// Round `addr` up to a page boundary (`None` on overflow).
pub const fn page_ceil(addr: u64) -> Option<u64> {
    match addr.checked_add(PAGE_SIZE - 1) {
        Some(v) => Some(v & !(PAGE_SIZE - 1)),
        None => None,
    }
}

/// `true` if `addr` is a canonical 48-bit virtual address (bits 48..63 copy bit 47).
pub const fn is_canonical(addr: u64) -> bool {
    let top = addr >> 47;
    top == 0 || top == 0x1_ffff
}

/// Table indices of `vaddr`, top level first: `[PML4, PDPT, PD, PT]`.
pub const fn table_indices(vaddr: u64) -> [usize; 4] {
    [
        ((vaddr >> 39) & 0x1ff) as usize,
        ((vaddr >> 30) & 0x1ff) as usize,
        ((vaddr >> 21) & 0x1ff) as usize,
        ((vaddr >> 12) & 0x1ff) as usize,
    ]
}

/// What a table entry means to a walk at `level` (4 = PML4 .. 1 = PT).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Not present: nothing is mapped here.
    Absent,
    /// Maps a 1 GiB (level 3) or 2 MiB (level 2) page: there is no 4 KiB entry to edit.
    Huge,
    /// Points to the next table, at this physical address.
    Table(u64),
    /// A level-1 entry: the 4 KiB mapping itself (physical frame address).
    Leaf(u64),
}

/// Interpret `entry` found at `level` (4..=1) of a walk.
pub const fn step(level: u8, entry: u64) -> Step {
    if entry & ENTRY_PRESENT == 0 {
        return Step::Absent;
    }
    let phys = entry & ENTRY_ADDR_MASK;
    match level {
        1 => Step::Leaf(phys),
        // Bit 7 is reserved (must be 0) in a PML4 entry; only levels 3 and 2 can be huge.
        3 | 2 if entry & ENTRY_HUGE != 0 => Step::Huge,
        _ => Step::Table(phys),
    }
}

/// `entry` with the `PRESENT` bit cleared (everything else kept, so the mapping can be restored).
pub const fn without_present(entry: u64) -> u64 {
    entry & !ENTRY_PRESENT
}

/// Bytes of the page-aligned block that holds a stack of `usable` bytes plus its guard page below
/// (`usable` rounded up to whole pages). `None` if `usable` is zero or the sum overflows.
pub const fn guarded_block_size(usable: usize) -> Option<usize> {
    if usable == 0 {
        return None;
    }
    let page = PAGE_SIZE as usize;
    let Some(rounded) = usable.checked_add(page - 1) else {
        return None;
    };
    match (rounded / page).checked_add(1) {
        Some(pages) => pages.checked_mul(page),
        None => None,
    }
}

/// Where the parts of a guarded stack block sit in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StackRegions {
    /// First byte of the guard page (the block's base).
    pub guard_start: u64,
    /// Lowest usable stack address (just above the guard page).
    pub stack_bottom: u64,
    /// One past the highest stack byte (the initial stack pointer is derived from it).
    pub stack_top: u64,
}

/// Split a block that starts at `base` (page aligned) and holds a stack of `usable` bytes plus a
/// guard page. `None` if `base` is not page aligned, `usable` is zero, or the addresses overflow.
pub fn stack_regions(base: u64, usable: usize) -> Option<StackRegions> {
    if !base.is_multiple_of(PAGE_SIZE) {
        return None;
    }
    let size = guarded_block_size(usable)? as u64;
    let stack_bottom = base.checked_add(PAGE_SIZE)?;
    let stack_top = base.checked_add(size)?;
    Some(StackRegions {
        guard_start: base,
        stack_bottom,
        stack_top,
    })
}

/// `true` if `addr` lies in the one-page guard that starts at `guard_start`.
pub const fn guard_hit(addr: u64, guard_start: u64) -> bool {
    addr >= guard_start && addr - guard_start < PAGE_SIZE
}

/// The stack pointer a fresh thread starts with: the top of its stack, 16-aligned, minus one word
/// (so `rsp ≡ 8 (mod 16)`, as if the entry function had just been called).
pub const fn initial_rsp(stack_top: u64) -> u64 {
    (stack_top & !0xF) - 8
}

#[cfg(test)]
mod tests;
