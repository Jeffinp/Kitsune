//! Minimal access to the active page tables, for thread-stack guard pages.
//!
//! The kernel does not own its page tables: the bootloader built them and maps all
//! physical memory at `physical_memory_offset` (`Mapping::Dynamic`, see `main.rs`).
//! That mapping is enough to *edit* them: walk from CR3 to the level-1 entry of a
//! page and clear its `PRESENT` bit (then `invlpg`), which turns the page into one
//! whose every access is a #PF. The heap, where thread stacks live, is part of the
//! kernel image's `.bss`, which the bootloader maps with 4 KiB pages, so one page can
//! be unmapped without splitting anything; if a walk ever meets a 2 MiB/1 GiB page the
//! functions here refuse ([`Error::HugePage`]) rather than edit a mapping that covers
//! more than the page asked for.
//!
//! Which entry to read at each level, and whether it can be descended into, is
//! decided by `kitsune_core::paging` (tested on the host); this file only does the
//! unsafe memory access.

use core::sync::atomic::{AtomicU64, Ordering};
use kitsune_core::paging::{self, PAGE_SIZE, Step};
use x86_64::VirtAddr;
use x86_64::registers::control::Cr3;

/// Virtual address of physical address 0 (0 until [`init`] gets one; no kernel mapping starts there).
static PHYS_OFFSET: AtomicU64 = AtomicU64::new(0);

/// Why a page-table operation was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The bootloader gave no `physical_memory_offset`, so the tables cannot be reached.
    NoPhysicalMap,
    /// The address is not canonical.
    NotCanonical,
    /// The page is not mapped.
    NotMapped,
    /// The page sits inside a 2 MiB / 1 GiB mapping; there is no 4 KiB entry to edit.
    HugePage,
}

/// Remember where the bootloader mapped physical memory. Call once at boot, before spawning threads.
pub fn init(phys_offset: Option<u64>) {
    PHYS_OFFSET.store(phys_offset.unwrap_or(0), Ordering::Relaxed);
}

/// Where the bootloader mapped physical memory (0 before [`init`]).
pub fn phys_offset() -> u64 {
    PHYS_OFFSET.load(Ordering::Relaxed)
}

/// Undo [`unmap_page`]: set `PRESENT` again on the page's entry (the rest of it was kept).
pub fn remap_page(page: u64) -> Result<(), Error> {
    debug_assert!(page.is_multiple_of(PAGE_SIZE));
    let pte = pte_ptr_any(page)?;
    // SAFETY: `pte` is the live level-1 entry of `page`; only the `PRESENT` bit is set, restoring the
    // mapping `unmap_page` removed, and the TLB entry is flushed so the change is seen at once.
    unsafe { pte.write_volatile(pte.read_volatile() | paging::ENTRY_PRESENT) };
    x86_64::instructions::tlb::flush(VirtAddr::new(page));
    Ok(())
}

/// Pointer to the level-1 entry that maps `vaddr`, found by walking the active tables from CR3.
fn pte_ptr(vaddr: u64) -> Result<*mut u64, Error> {
    let offset = PHYS_OFFSET.load(Ordering::Relaxed);
    if offset == 0 {
        return Err(Error::NoPhysicalMap);
    }
    if !paging::is_canonical(vaddr) {
        return Err(Error::NotCanonical);
    }
    let mut table = Cr3::read().0.start_address().as_u64();
    for (i, &index) in paging::table_indices(vaddr).iter().enumerate() {
        let level = 4 - i as u8;
        // SAFETY: `table` is the physical address of a live page table (the CR3 frame, or the frame a
        // present non-huge entry pointed to), the bootloader maps all physical memory at `offset`, and
        // `index < 512` keeps the access inside that 4 KiB table. Only read here.
        let entry_ptr = unsafe { ((offset + table) as *mut u64).add(index) };
        // SAFETY: `entry_ptr` was just derived from a valid table (see above).
        let entry = unsafe { entry_ptr.read_volatile() };
        match paging::step(level, entry) {
            Step::Absent => return Err(Error::NotMapped),
            Step::Huge => return Err(Error::HugePage),
            Step::Table(next) => table = next,
            Step::Leaf(_) => return Ok(entry_ptr),
        }
    }
    Err(Error::NotMapped) // unreachable: level 1 yields `Leaf` or `Absent`
}

/// Like [`pte_ptr`], but also finds the level-1 entry of a page whose `PRESENT` bit is clear.
fn pte_ptr_any(vaddr: u64) -> Result<*mut u64, Error> {
    let offset = PHYS_OFFSET.load(Ordering::Relaxed);
    if offset == 0 {
        return Err(Error::NoPhysicalMap);
    }
    if !paging::is_canonical(vaddr) {
        return Err(Error::NotCanonical);
    }
    let mut table = Cr3::read().0.start_address().as_u64();
    for (i, &index) in paging::table_indices(vaddr).iter().enumerate() {
        let level = 4 - i as u8;
        // SAFETY: as in `pte_ptr`: `table` is a live page table reached through the physical map and
        // `index < 512`; only read here.
        let entry_ptr = unsafe { ((offset + table) as *mut u64).add(index) };
        if level == 1 {
            return Ok(entry_ptr);
        }
        // SAFETY: as above.
        let entry = unsafe { entry_ptr.read_volatile() };
        match paging::step(level, entry) {
            Step::Table(next) => table = next,
            Step::Absent => return Err(Error::NotMapped),
            Step::Huge => return Err(Error::HugePage),
            Step::Leaf(_) => return Ok(entry_ptr),
        }
    }
    Err(Error::NotMapped)
}

/// Is the 4 KiB page containing `vaddr` mapped?
pub fn is_mapped(vaddr: u64) -> Result<bool, Error> {
    match pte_ptr(vaddr) {
        Ok(_) => Ok(true),
        Err(Error::NotMapped) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Make the page at `page` (page aligned) inaccessible: clear `PRESENT` in its entry and flush the TLB
/// entry. The rest of the entry is kept, so the mapping could be restored by setting the bit again.
///
/// The caller must own the page and nothing may read or write it afterwards, other than to fault.
pub fn unmap_page(page: u64) -> Result<(), Error> {
    debug_assert!(page.is_multiple_of(PAGE_SIZE));
    let pte = pte_ptr(page)?;
    // SAFETY: `pte` points at the live level-1 entry of `page` (from `pte_ptr`); rewriting it with only
    // `PRESENT` cleared changes nothing else about the mapping, and the caller guarantees nobody uses
    // the page. The TLB entry is flushed right after, so no stale translation survives.
    unsafe { pte.write_volatile(paging::without_present(pte.read_volatile())) };
    x86_64::instructions::tlb::flush(VirtAddr::new(page));
    Ok(())
}

/// Most pages [`find_guard_below`] looks through (16 MiB of stack is far beyond any thread here).
const MAX_STACK_PAGES: usize = 4096;

/// First unmapped page at or below the page of `sp`, walking down page by page: for a running stack this
/// is the guard page the bootloader left under it. `None` if the tables are unreachable, a huge page is in
/// the way, or no hole shows up within [`MAX_STACK_PAGES`].
pub fn find_guard_below(sp: u64) -> Option<u64> {
    let mut page = paging::page_floor(sp);
    for _ in 0..MAX_STACK_PAGES {
        match is_mapped(page) {
            Ok(true) => page = page.checked_sub(PAGE_SIZE)?,
            Ok(false) => return Some(page),
            Err(_) => return None,
        }
    }
    None
}
