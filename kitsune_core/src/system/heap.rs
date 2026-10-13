//! Pure arithmetic for a linked-list heap allocator. The error-prone part —
//! alignment and region-fit math — lives here and is unit-tested. The kernel
//! provides the thin `unsafe` glue that writes free-list nodes into memory.
//! Small blocks come from size classes ([`slab_class`]): slabs of [`SLAB_SIZE`] bytes cut into
//! blocks of one size, so the list only holds the big ones.

/// Round `addr` up to a multiple of `align` (which must be a power of two).
pub const fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

/// True when `align` is a non-zero power of two.
pub const fn is_power_of_two(align: usize) -> bool {
    align != 0 && (align & (align - 1)) == 0
}

/// Try to place an allocation of `size` bytes aligned to `align` inside the
/// free region `[region_start, region_start + region_size)`.
///
/// Returns `(alloc_start, excess)` where `excess` is the leftover tail after
/// the allocation. The fit is rejected when it doesn't physically fit, or when
/// the leftover is non-zero but too small to hold a free-list node
/// (`min_block`) — that would strand unrecoverable memory.
pub fn fit_region(
    region_start: usize,
    region_size: usize,
    size: usize,
    align: usize,
    min_block: usize,
) -> Option<(usize, usize)> {
    let alloc_start = align_up(region_start, align);
    let alloc_end = alloc_start.checked_add(size)?;
    let region_end = region_start.checked_add(region_size)?;
    if alloc_end > region_end {
        return None;
    }
    let excess = region_end - alloc_end;
    if excess > 0 && excess < min_block {
        return None;
    }
    Some((alloc_start, excess))
}

/// A placement inside a free region that accounts for *every* byte of it:
/// `front + size + excess == region_size`. Both `front` and `excess` are either
/// zero or at least `min_block`, so each leftover can go back on the free list
/// as a node instead of being stranded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fit {
    /// Bytes between the region start and the (aligned) allocation.
    pub front: usize,
    pub alloc_start: usize,
    /// Bytes between the end of the allocation and the region end.
    pub excess: usize,
}

/// Like [`fit_region`], but also reports the padding *in front of* the
/// allocation so the caller can return it to the free list, instead of leaking
/// it (`fit_region` only reports the tail).
///
/// When aligning would leave a front gap too small to hold a free-list node
/// (`0 < front < min_block`), the allocation is moved to the next aligned
/// address, which makes the gap large enough. Returns `None` if nothing fits
/// (including when any address computation would overflow).
#[inline]
pub fn fit_region_split(
    region_start: usize,
    region_size: usize,
    size: usize,
    align: usize,
    min_block: usize,
) -> Option<Fit> {
    // Cheap early-out for the common case while scanning a long free list.
    if region_size < size || !is_power_of_two(align) {
        return None;
    }
    let region_end = region_start.checked_add(region_size)?;
    let mut alloc_start = region_start.checked_add(align - 1)? & !(align - 1);
    if alloc_start != region_start && alloc_start - region_start < min_block {
        alloc_start = alloc_start.checked_add(align)?;
    }
    let alloc_end = alloc_start.checked_add(size)?;
    if alloc_end > region_end {
        return None;
    }
    let excess = region_end - alloc_end;
    if excess > 0 && excess < min_block {
        return None;
    }
    Some(Fit {
        front: alloc_start - region_start,
        alloc_start,
        excess,
    })
}

/// True when the free region `[a_start, a_start + a_size)` ends exactly where
/// `b_start` begins — i.e. the two are contiguous and can be merged into one.
/// Overflow-safe (a wrapping end can never legitimately touch `b_start`).
pub const fn regions_adjacent(a_start: usize, a_size: usize, b_start: usize) -> bool {
    match a_start.checked_add(a_size) {
        Some(end) => end == b_start,
        None => false,
    }
}

/// Normalize a requested `(size, align)` so every allocation is at least large
/// enough — and aligned enough — to host a free-list node once freed.
pub fn adjust_request(
    size: usize,
    align: usize,
    node_size: usize,
    node_align: usize,
) -> (usize, usize) {
    let align = align.max(node_align);
    let size = align_up(size.max(node_size), node_align);
    (size, align)
}

// ---- size classes (slabs) ----

/// Bytes of one slab: a chunk of the heap, aligned to its size, cut into blocks of one size class.
/// The owner of a small block is found by masking its address.
pub const SLAB_SIZE: usize = 64 * 1024;
/// Bytes at the start of a slab kept for its header (the blocks start after it, 16-aligned).
pub const SLAB_HEADER: usize = 64;
/// The block sizes of the classes: steps of 16 up to 128, then four classes per doubling.
pub const SLAB_CLASSES: [usize; 24] = [
    16, 32, 48, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 640, 768, 896, 1024,
    1280, 1536, 1792, 2048,
];
/// The largest request a slab serves; bigger ones go to the linked list.
pub const SLAB_MAX: usize = 2048;

/// The class for a request of `size` bytes aligned to `align`, or `None` when it goes to the list
/// (too big, or aligned beyond 16). A zero size takes the smallest class.
pub fn slab_class(size: usize, align: usize) -> Option<usize> {
    if align > 16 || size > SLAB_MAX {
        return None;
    }
    let size = size.max(1);
    Some(if size <= 128 {
        (size - 1) / 16
    } else {
        // Four classes per doubling above 128: 160, 192, 224, 256, 320, ...
        let top = usize::BITS - 1 - (size - 1).leading_zeros(); // 2^top <= size-1 < 2^(top+1)
        let step = 1usize << (top - 2);
        let base = 1usize << top;
        let k = (size - 1 - base) / step; // 0..=3
        8 + (top as usize - 7) * 4 + k
    })
}

/// How many blocks of class `c` a slab holds.
pub const fn slab_blocks(c: usize) -> usize {
    (SLAB_SIZE - SLAB_HEADER) / SLAB_CLASSES[c]
}

#[cfg(test)]
mod tests;
