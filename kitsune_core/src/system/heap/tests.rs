use super::*;

#[test]
fn align_up_basic() {
    assert_eq!(align_up(0, 8), 0);
    assert_eq!(align_up(1, 8), 8);
    assert_eq!(align_up(8, 8), 8);
    assert_eq!(align_up(9, 8), 16);
    assert_eq!(align_up(100, 64), 128);
}

#[test]
fn power_of_two_check() {
    assert!(is_power_of_two(1));
    assert!(is_power_of_two(8));
    assert!(is_power_of_two(4096));
    assert!(!is_power_of_two(0));
    assert!(!is_power_of_two(3));
    assert!(!is_power_of_two(48));
}

#[test]
fn fit_exact() {
    // Region [0,100), request 100 aligned 1 -> fits, no excess.
    assert_eq!(fit_region(0, 100, 100, 1, 16), Some((0, 0)));
}

#[test]
fn fit_with_recoverable_excess() {
    // 100 - 40 = 60 leftover >= min_block(16) -> ok.
    assert_eq!(fit_region(0, 100, 40, 1, 16), Some((0, 60)));
}

#[test]
fn reject_excess_too_small_to_hold_node() {
    // leftover 10 < min_block 16 -> stranded -> reject.
    assert_eq!(fit_region(0, 50, 40, 1, 16), None);
}

#[test]
fn fit_respects_alignment_padding() {
    // Region starts at 5, align 8 -> alloc_start 8, needs 8+16=24 <= 5+40=45.
    let (start, excess) = fit_region(5, 40, 16, 8, 8).unwrap();
    assert_eq!(start, 8);
    assert_eq!(start % 8, 0);
    assert_eq!(excess, 45 - 24);
}

#[test]
fn reject_when_too_big() {
    assert_eq!(fit_region(0, 32, 64, 1, 16), None);
}

#[test]
fn reject_when_alignment_pushes_past_end() {
    // Region [10, 16): aligning to 64 -> 64, way past end.
    assert_eq!(fit_region(10, 6, 1, 64, 16), None);
}

#[test]
fn fit_overflow_is_safe() {
    assert_eq!(fit_region(usize::MAX - 4, 8, 16, 1, 16), None);
}

#[test]
fn adjacent_regions_merge() {
    // [0,64) ends at 64, where [64,..) begins -> adjacent.
    assert!(regions_adjacent(0, 64, 64));
    assert!(regions_adjacent(4096, 16, 4112));
}

#[test]
fn non_adjacent_regions_dont_merge() {
    // Gap between end (63) and next start (64).
    assert!(!regions_adjacent(0, 63, 64));
    // Overlap / out of order.
    assert!(!regions_adjacent(0, 100, 64));
    assert!(!regions_adjacent(128, 16, 64));
}

#[test]
fn adjacent_overflow_is_safe() {
    assert!(!regions_adjacent(usize::MAX - 4, 8, 0));
}

// ---- fit_region_split ----

const MIN: usize = 16;

#[test]
fn split_fit_aligned_region_has_no_front() {
    let f = fit_region_split(4096, 8192, 100, 8, MIN).unwrap();
    assert_eq!(f.front, 0);
    assert_eq!(f.alloc_start, 4096);
    assert_eq!(f.front + 100 + f.excess, 8192);
}

#[test]
fn split_fit_reports_large_front_padding() {
    // The audit's case: region at 8 mod 4096, request aligned to 4096.
    let f = fit_region_split(4104, 8192, 16, 4096, MIN).unwrap();
    assert_eq!(f.alloc_start, 8192);
    assert_eq!(f.front, 4088);
    assert_eq!(f.front + 16 + f.excess, 8192); // nothing lost
    // The old API hid `front`:
    assert_eq!(
        fit_region(4104, 8192, 16, 4096, MIN).map(|(s, _)| s),
        Some(8192)
    );
}

#[test]
fn split_fit_skips_to_next_slot_when_front_cannot_hold_a_node() {
    // Region at 8: aligning to 16 leaves an 8-byte gap (< 16, no room for a
    // node), so the allocation moves to 24 and the gap becomes 16.
    let f = fit_region_split(8, 200, 32, 16, MIN).unwrap();
    assert_eq!(f.alloc_start, 32);
    assert_eq!(f.front, 24);
    assert_eq!(f.alloc_start % 16, 0);
    assert_eq!(f.front + 32 + f.excess, 200);
}

#[test]
fn split_fit_rejects_when_the_shifted_slot_does_not_fit() {
    assert_eq!(fit_region_split(8, 40, 32, 16, MIN), None);
    // Fits only without the shift: [8, 8+32) needs alignment 8.
    assert!(fit_region_split(8, 32, 32, 8, MIN).is_some());
}

#[test]
fn split_fit_rejects_stranded_tail_like_fit_region() {
    assert_eq!(fit_region_split(0, 50, 40, 1, MIN), None);
    assert_eq!(
        fit_region_split(0, 56, 40, 1, MIN).map(|f| f.excess),
        Some(16)
    );
    assert_eq!(
        fit_region_split(0, 40, 40, 1, MIN).map(|f| f.excess),
        Some(0)
    );
}

#[test]
fn split_fit_rejects_bad_alignment_and_overflow() {
    assert_eq!(fit_region_split(0, 100, 8, 0, MIN), None);
    assert_eq!(fit_region_split(0, 100, 8, 24, MIN), None); // not a power of two
    assert_eq!(fit_region_split(usize::MAX - 4, 8, 16, 1, MIN), None);
    assert_eq!(fit_region_split(usize::MAX - 8, 8, 1, 4096, MIN), None); // align_up would wrap
    assert_eq!(fit_region_split(0, 100, usize::MAX, 8, MIN), None);
}

/// Address-sorted free list model mirroring the kernel's allocator: find the
/// first region that fits, unlink it whole, give back front and tail, and
/// coalesce neighbours.
struct Model {
    free: Vec<(usize, usize)>, // (start, size), sorted by start
    use_split: bool,
}

impl Model {
    fn insert(&mut self, start: usize, size: usize) {
        let i = self.free.partition_point(|&(s, _)| s < start);
        self.free.insert(i, (start, size));
        // Merge with the successor, then the predecessor.
        if i + 1 < self.free.len() && regions_adjacent(start, size, self.free[i + 1].0) {
            self.free[i].1 += self.free[i + 1].1;
            self.free.remove(i + 1);
        }
        if i > 0 && regions_adjacent(self.free[i - 1].0, self.free[i - 1].1, self.free[i].0) {
            self.free[i - 1].1 += self.free[i].1;
            self.free.remove(i);
        }
    }

    fn alloc(&mut self, size: usize, align: usize) -> Option<(usize, usize)> {
        let (size, align) = adjust_request(size, align, MIN, 8);
        for i in 0..self.free.len() {
            let (rs, rsz) = self.free[i];
            let (alloc_start, front) = if self.use_split {
                match fit_region_split(rs, rsz, size, align, MIN) {
                    Some(f) => (f.alloc_start, f.front),
                    None => continue,
                }
            } else {
                match fit_region(rs, rsz, size, align, MIN) {
                    Some((a, _)) => (a, 0), // old kernel: front gap dropped
                    None => continue,
                }
            };
            self.free.remove(i);
            let end = alloc_start + size;
            if front > 0 {
                self.insert(rs, front);
            }
            if rs + rsz > end {
                self.insert(end, rs + rsz - end);
            }
            return Some((alloc_start, size));
        }
        None
    }

    fn free_bytes(&self) -> usize {
        self.free.iter().map(|&(_, s)| s).sum()
    }
}

fn run_fuzz(use_split: bool) -> (usize, usize) {
    const BASE: usize = 0x1000_0008; // deliberately not page aligned
    const SIZE: usize = 1 << 20;
    let mut m = Model {
        free: vec![(BASE, SIZE)],
        use_split,
    };
    let mut rng = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let mut live: Vec<(usize, usize, usize)> = Vec::new(); // (start, size, align)
    for _ in 0..20_000 {
        if live.len() < 40 && next() % 3 != 0 {
            let size = 1 + (next() % 3000) as usize;
            let align = [8usize, 16, 64, 4096][(next() % 4) as usize];
            if let Some((start, sz)) = m.alloc(size, align) {
                assert_eq!(start % align.max(8), 0, "misaligned allocation");
                for &(s, z, _) in &live {
                    assert!(start + sz <= s || s + z <= start, "overlapping allocations");
                }
                live.push((start, sz, align));
            }
        } else if !live.is_empty() {
            let i = (next() as usize) % live.len();
            let (s, z, _) = live.swap_remove(i);
            m.insert(s, z);
        }
    }
    for (s, z, _) in live.drain(..) {
        m.insert(s, z);
    }
    (m.free_bytes(), m.free.len())
}

#[test]
fn model_with_split_fit_never_leaks_front_padding() {
    let (bytes, nodes) = run_fuzz(true);
    assert_eq!(bytes, 1 << 20, "bytes leaked");
    assert_eq!(nodes, 1, "heap did not coalesce back to one region");
}

#[test]
fn model_with_old_fit_region_leaks_as_documented_in_the_audit() {
    // Documents the bug the split fit fixes (docs/audit/01-memoria-unsafe.md #5).
    let (bytes, nodes) = run_fuzz(false);
    assert!(bytes < 1 << 20, "expected a leak, free = {bytes}");
    assert!(nodes > 1);
}

#[test]
fn adjust_request_enforces_node_minimums() {
    // Tiny request grows to at least node_size and node_align.
    let (size, align) = adjust_request(1, 1, 16, 8);
    assert_eq!(size, 16);
    assert_eq!(align, 8);
}

#[test]
fn adjust_request_keeps_larger_values() {
    let (size, align) = adjust_request(100, 64, 16, 8);
    assert_eq!(size, align_up(100, 8));
    assert_eq!(align, 64);
}

#[test]
fn slab_classes_fit_every_small_request() {
    for size in 0..=SLAB_MAX {
        for align in [1, 2, 4, 8, 16] {
            let c = slab_class(size, align).expect("small requests have a class");
            assert!(SLAB_CLASSES[c] >= size.max(1), "size {size} class {c}");
            // The smallest class that fits.
            assert!(
                c == 0 || SLAB_CLASSES[c - 1] < size.max(1),
                "size {size} class {c}"
            );
        }
    }
    assert_eq!(slab_class(SLAB_MAX + 1, 8), None);
    assert_eq!(slab_class(64, 32), None);
    assert_eq!(slab_class(0, 1), Some(0));
    assert_eq!(slab_class(129, 8), Some(8));
    assert_eq!(slab_class(2048, 16), Some(23));
}

#[test]
fn slab_classes_are_16_aligned_and_fill_a_slab() {
    for (c, &size) in SLAB_CLASSES.iter().enumerate() {
        assert_eq!(size % 16, 0);
        assert!(slab_blocks(c) >= 31, "class {c} holds {}", slab_blocks(c));
        assert!(SLAB_HEADER + slab_blocks(c) * size <= SLAB_SIZE);
    }
    assert!(SLAB_CLASSES.windows(2).all(|w| w[0] < w[1]));
    assert!(SLAB_SIZE.is_power_of_two() && SLAB_HEADER % 16 == 0);
}
