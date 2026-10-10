use super::*;

const MIB: u64 = 1 << 20;
const P: u64 = PAGE_SIZE;

#[test]
fn alloc_order_and_counts() {
    let mut a = FrameAlloc::new(&[(2 * MIB, 2 * MIB + 3 * P)]);
    assert_eq!(a.total(), 3);
    assert_eq!(a.used(), 0);
    assert_eq!(a.alloc(), Some(2 * MIB));
    assert_eq!(a.alloc(), Some(2 * MIB + P));
    assert_eq!(a.alloc(), Some(2 * MIB + 2 * P));
    assert_eq!(a.used(), 3);
    assert_eq!(a.alloc(), None);
}

#[test]
fn recycling_prefers_freed_frames() {
    let mut a = FrameAlloc::new(&[(2 * MIB, 2 * MIB + 4 * P)]);
    let f0 = a.alloc().unwrap();
    let f1 = a.alloc().unwrap();
    assert_eq!(a.free(f1), Ok(()));
    assert_eq!(a.free(f0), Ok(()));
    assert_eq!(a.used(), 0);
    assert_eq!(a.alloc(), Some(f0));
    assert_eq!(a.alloc(), Some(f1));
    assert_eq!(a.alloc(), Some(2 * MIB + 2 * P));
    assert_eq!(a.used(), 3);
}

#[test]
fn exhaustion_then_free_then_alloc() {
    let mut a = FrameAlloc::new(&[(MIB, MIB + 2 * P)]);
    let f = a.alloc().unwrap();
    a.alloc().unwrap();
    assert_eq!(a.alloc(), None);
    assert_eq!(a.alloc(), None);
    a.free(f).unwrap();
    assert_eq!(a.alloc(), Some(f));
    assert_eq!(a.alloc(), None);
}

#[test]
fn free_errors() {
    let mut a = FrameAlloc::new(&[(2 * MIB, 2 * MIB + 4 * P)]);
    let f = a.alloc().unwrap();
    assert_eq!(a.free(f + 1), Err(FrameError::Misaligned));
    assert_eq!(a.free(0), Err(FrameError::OutOfRange));
    assert_eq!(a.free(2 * MIB - P), Err(FrameError::OutOfRange));
    assert_eq!(a.free(2 * MIB + 4 * P), Err(FrameError::OutOfRange));
    assert_eq!(a.free(!(P - 1)), Err(FrameError::OutOfRange));
    assert_eq!(a.free(u64::MAX), Err(FrameError::Misaligned));
    // Inside the region but beyond the bump pointer: never handed out.
    assert_eq!(a.free(2 * MIB + 2 * P), Err(FrameError::NotAllocated));
    assert_eq!(a.used(), 1);
    assert_eq!(a.free(f), Ok(()));
    assert_eq!(a.free(f), Err(FrameError::NotAllocated));
    assert_eq!(a.used(), 0);
}

#[test]
fn gap_between_regions_is_out_of_range() {
    let mut a = FrameAlloc::new(&[(2 * MIB, 2 * MIB + P), (8 * MIB, 8 * MIB + P)]);
    assert_eq!(a.alloc(), Some(2 * MIB));
    assert_eq!(a.alloc(), Some(8 * MIB));
    assert_eq!(a.free(4 * MIB), Err(FrameError::OutOfRange));
    assert_eq!(a.free(8 * MIB), Ok(()));
    assert_eq!(a.free(2 * MIB), Ok(()));
}

#[test]
fn regions_are_trimmed_inward() {
    // Start rounds up, end rounds down: only one whole page remains.
    let a = FrameAlloc::new(&[(2 * MIB + 1, 2 * MIB + 2 * P + 5)]);
    assert_eq!(a.total(), 1);
    let mut a = a;
    assert_eq!(a.alloc(), Some(2 * MIB + P));
    assert_eq!(a.alloc(), None);
}

#[test]
fn low_memory_is_dropped() {
    assert_eq!(FrameAlloc::new(&[(0, MIB)]).total(), 0);
    assert_eq!(FrameAlloc::new(&[(0, MIB - 1)]).total(), 0);
    let mut a = FrameAlloc::new(&[(0, MIB + 2 * P)]);
    assert_eq!(a.total(), 2);
    assert_eq!(a.alloc(), Some(MIB));
    assert_eq!(a.free(0), Err(FrameError::OutOfRange));
}

#[test]
fn empty_and_degenerate_input() {
    assert_eq!(FrameAlloc::new(&[]).total(), 0);
    assert_eq!(FrameAlloc::new(&[(5 * MIB, 5 * MIB)]).total(), 0);
    assert_eq!(FrameAlloc::new(&[(6 * MIB, 5 * MIB)]).total(), 0);
    assert_eq!(FrameAlloc::new(&[(u64::MAX - 10, u64::MAX)]).total(), 0);
    assert_eq!(FrameAlloc::new(&[(2 * MIB, 2 * MIB + P - 1)]).total(), 0);
    assert_eq!(FrameAlloc::new(&[]).alloc(), None);
}

#[test]
fn unsorted_overlapping_and_adjacent_regions_merge() {
    let mut a = FrameAlloc::new(&[
        (6 * MIB, 6 * MIB + 2 * P),
        (2 * MIB, 2 * MIB + 3 * P),
        (2 * MIB + 2 * P, 2 * MIB + 5 * P), // overlaps the previous one
        (2 * MIB + 5 * P, 2 * MIB + 6 * P), // adjacent
        (3 * MIB, 3 * MIB),                 // empty
    ]);
    assert_eq!(a.total(), 6 + 2);
    for i in 0..6 {
        assert_eq!(a.alloc(), Some(2 * MIB + i * P));
    }
    assert_eq!(a.alloc(), Some(6 * MIB));
    assert_eq!(a.alloc(), Some(6 * MIB + P));
    assert_eq!(a.alloc(), None);
    // The seam of the merged region is one region: freeing across it works.
    assert_eq!(a.free(2 * MIB + 2 * P), Ok(()));
    assert_eq!(a.free(2 * MIB + 2 * P), Err(FrameError::NotAllocated));
}

#[test]
fn huge_region_costs_no_memory() {
    let tib: u64 = 1 << 40;
    let mut a = FrameAlloc::new(&[(MIB, 2 * tib)]);
    assert_eq!(a.total() as u64, (2 * tib - MIB) / P);
    let f = a.alloc().unwrap();
    assert_eq!(f, MIB);
    let g = a.alloc().unwrap();
    assert_eq!(a.used(), 2);
    assert_eq!(a.free(2 * tib - P), Err(FrameError::NotAllocated));
    assert_eq!(a.free(g), Ok(()));
    assert_eq!(a.free(f), Ok(()));
    assert_eq!(a.alloc(), Some(f));
}

#[test]
fn many_cycles_keep_counts_consistent() {
    let mut a = FrameAlloc::new(&[(2 * MIB, 2 * MIB + 64 * P)]);
    let mut held = Vec::new();
    for round in 0..200u64 {
        while let Some(f) = a.alloc() {
            assert!(!held.contains(&f));
            held.push(f);
            if held.len() as u64 > 10 + round % 50 {
                break;
            }
        }
        let n = held.len() / 2;
        for f in held.drain(..n) {
            assert_eq!(a.free(f), Ok(()));
        }
        assert_eq!(a.used(), held.len());
        assert!(a.used() <= a.total());
    }
}
