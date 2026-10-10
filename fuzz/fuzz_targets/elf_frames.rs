//! Fuzz target: user-process ELF loader and physical frame allocator.
//!
//! * `elf::parse` on arbitrary bytes with arbitrary bounds never panics, and a returned
//!   `Program` keeps its promises: every segment inside the bounds, no writable + executable
//!   segment, the entry inside an executable segment, no two segments sharing a page, the
//!   file ranges inside the input and the memory total within the limit;
//! * `FrameAlloc` driven by a sequence of alloc/free operations (frees of arbitrary
//!   addresses): `used() <= total()`, every frame returned is page-aligned, inside a region and
//!   never handed out twice while it is still allocated, and `free` succeeds exactly for the
//!   frames that are allocated.
#![no_main]

use std::collections::BTreeSet;

use kitsune_core::elf;
use kitsune_core::paging::{FrameAlloc, FrameError, PAGE_SIZE};
use libfuzzer_sys::fuzz_target;

fn u64_at(d: &[u8], i: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[i..i + 8]);
    u64::from_le_bytes(b)
}

fn elf_case(data: &[u8]) {
    if data.len() < 24 {
        return;
    }
    let (mut lo, mut hi, mut max) = (u64_at(data, 0), u64_at(data, 8), u64_at(data, 16));
    // Often use realistic bounds so the parser gets past the range checks.
    match data[0] & 3 {
        0 => {
            lo = 0x40_0000;
            hi = 0x7fff_0000_0000;
        }
        1 => max >>= 20,
        _ => {}
    }
    let image = &data[24..];
    let Ok(prog) = elf::parse(image, lo, hi, max) else {
        return;
    };
    assert!(prog.segments.len() <= 16);
    let mut total = 0u64;
    let mut entry_ok = false;
    for s in &prog.segments {
        assert!(s.mem_size > 0);
        assert!(s.vaddr >= lo);
        assert!(s.vaddr.checked_add(s.mem_size).unwrap() <= hi);
        assert!(!(s.write && s.exec));
        assert!(s.file_size as u64 <= s.mem_size);
        assert!(s.file_off.checked_add(s.file_size).unwrap() <= image.len());
        assert_eq!(s.vaddr % 4096, s.file_off as u64 % 4096);
        total += s.mem_size;
        if s.exec && prog.entry >= s.vaddr && prog.entry - s.vaddr < s.mem_size {
            entry_ok = true;
        }
    }
    assert!(total <= max);
    assert!(entry_ok);
    // No page shared between two segments (any order).
    let pages = |s: &elf::Segment| {
        let first = s.vaddr / 4096;
        let last = (s.vaddr + s.mem_size - 1) / 4096;
        (first, last)
    };
    for (i, a) in prog.segments.iter().enumerate() {
        for b in &prog.segments[i + 1..] {
            let (a0, a1) = pages(a);
            let (b0, b1) = pages(b);
            assert!(a1 < b0 || b1 < a0);
        }
    }
}

fn frames_case(data: &[u8]) {
    // Up to 4 regions of 16 bytes, then 9-byte operations.
    let n = (data.first().copied().unwrap_or(0) & 3) as usize + 1;
    let mut regions = Vec::new();
    let mut pos = 1;
    for _ in 0..n {
        if pos + 16 > data.len() {
            break;
        }
        let (s, e) = (u64_at(data, pos), u64_at(data, pos + 8));
        pos += 16;
        // Keep them small and near 1 MiB so frees can hit them, sometimes huge.
        let s = (s % (1 << 24)) + (1 << 19);
        let e = if e & 7 == 0 { e } else { s + (e % (1 << 18)) };
        regions.push((s, e));
    }
    let mut a = FrameAlloc::new(&regions);
    let mut held: BTreeSet<u64> = BTreeSet::new();
    let mut ever: BTreeSet<u64> = BTreeSet::new();
    for op in data[pos.min(data.len())..].chunks_exact(9).take(4096) {
        let addr = u64_at(op, 1);
        if op[0] & 1 == 0 {
            if let Some(f) = a.alloc() {
                assert_eq!(f % PAGE_SIZE, 0);
                assert!(f >= 1 << 20);
                assert!(regions.iter().any(|&(s, e)| f >= s && f + PAGE_SIZE <= e));
                assert!(held.insert(f), "frame handed out twice");
                ever.insert(f);
            }
        } else {
            // Half the time free a real frame, otherwise an arbitrary address.
            let target = if op[0] & 2 != 0 {
                held.iter()
                    .nth((addr as usize) % held.len().max(1))
                    .copied()
                    .unwrap_or(addr)
            } else if op[0] & 4 != 0 {
                (addr % (1 << 25)) & !(PAGE_SIZE - 1)
            } else {
                addr
            };
            let r = a.free(target);
            if held.remove(&target) {
                assert_eq!(r, Ok(()));
            } else {
                assert!(r.is_err());
                if target % PAGE_SIZE != 0 {
                    assert_eq!(r, Err(FrameError::Misaligned));
                } else if ever.contains(&target) {
                    assert_eq!(r, Err(FrameError::NotAllocated));
                }
            }
        }
        assert!(a.used() <= a.total());
        assert_eq!(a.used(), held.len());
    }
}

fuzz_target!(|data: &[u8]| {
    elf_case(data);
    frames_case(data);
});
