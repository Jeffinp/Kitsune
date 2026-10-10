//! Physical frame allocator for user processes: hands out 4 KiB frames from the usable
//! regions the bootloader reports.
//!
//! Memory use does not depend on the size of the regions: each region keeps a *bump pointer*
//! (everything below it has been handed out at least once, everything at or above it never
//! was) and the frames returned by `free` wait in a `BTreeSet` until they are reused. That
//! makes double frees and frees of foreign frames exactly detectable without a bitmap.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use super::PAGE_SIZE;

/// Nothing below 1 MiB is ever handed out (real-mode area, BIOS, legacy devices).
const LOW_LIMIT: u64 = 1 << 20;

/// Why a frame could not be returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// The address is not a multiple of 4 KiB.
    Misaligned,
    /// The frame lies outside every region.
    OutOfRange,
    /// The frame was never handed out, or is already free (double free).
    NotAllocated,
}

#[derive(Debug, Clone, Copy)]
struct Region {
    start: u64,
    end: u64,
    /// First frame of the region never handed out (`start <= next <= end`).
    next: u64,
}

/// Frame allocator over a few usable physical regions.
#[derive(Debug, Clone)]
pub struct FrameAlloc {
    regions: Vec<Region>,
    free: BTreeSet<u64>,
    total: usize,
    used: usize,
}

impl FrameAlloc {
    /// `regions`: usable physical ranges `[start, end)` in bytes. Each is trimmed inward to
    /// whole 4 KiB pages and anything below 1 MiB is dropped; empty, overlapping and unsorted
    /// input is tolerated (the ranges are sorted and merged).
    pub fn new(regions: &[(u64, u64)]) -> FrameAlloc {
        let mut v: Vec<(u64, u64)> = Vec::new();
        for &(s, e) in regions {
            let s = s.max(LOW_LIMIT);
            let Some(s) = s.checked_add(PAGE_SIZE - 1).map(|s| s & !(PAGE_SIZE - 1)) else {
                continue;
            };
            let e = e & !(PAGE_SIZE - 1);
            if s < e {
                v.push((s, e));
            }
        }
        v.sort_unstable();
        let mut merged: Vec<Region> = Vec::new();
        for (s, e) in v {
            match merged.last_mut() {
                Some(last) if s <= last.end => last.end = last.end.max(e),
                _ => merged.push(Region {
                    start: s,
                    end: e,
                    next: s,
                }),
            }
        }
        let frames: u64 = merged.iter().map(|r| (r.end - r.start) / PAGE_SIZE).sum();
        FrameAlloc {
            regions: merged,
            free: BTreeSet::new(),
            total: usize::try_from(frames).unwrap_or(usize::MAX),
            used: 0,
        }
    }

    /// Next free frame (page-aligned physical address) or `None`. Recycled frames come first
    /// (lowest address first), then the regions are bumped through in order.
    pub fn alloc(&mut self) -> Option<u64> {
        if let Some(f) = self.free.pop_first() {
            self.used += 1;
            return Some(f);
        }
        for r in &mut self.regions {
            if r.next < r.end {
                let f = r.next;
                r.next += PAGE_SIZE;
                self.used += 1;
                return Some(f);
            }
        }
        None
    }

    /// Return a frame. Fails if it is misaligned, outside every region, never handed out or
    /// already free.
    pub fn free(&mut self, frame: u64) -> Result<(), FrameError> {
        if !frame.is_multiple_of(PAGE_SIZE) {
            return Err(FrameError::Misaligned);
        }
        let idx = self.regions.partition_point(|r| r.end <= frame);
        let r = self.regions.get(idx).ok_or(FrameError::OutOfRange)?;
        if frame < r.start {
            return Err(FrameError::OutOfRange);
        }
        if frame >= r.next || !self.free.insert(frame) {
            return Err(FrameError::NotAllocated);
        }
        self.used -= 1;
        Ok(())
    }

    /// Frames in all regions.
    pub fn total(&self) -> usize {
        self.total
    }

    /// Frames handed out and not freed.
    pub fn used(&self) -> usize {
        self.used
    }
}

#[cfg(test)]
mod tests;
