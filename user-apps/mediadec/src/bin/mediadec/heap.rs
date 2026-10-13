//! `malloc`, `calloc`, `realloc`, `free` and `malloc_usable_size` for the C side, over the SDK's
//! allocator (a heap grown with `brk`). Each block carries a 16-byte header with its size in front of
//! what C sees, so `free` knows the layout to give back and the pointer stays 16-byte aligned.

use core::alloc::Layout;
use core::ptr;

extern crate alloc;

const HDR: usize = 16;

fn layout(size: usize) -> Option<Layout> {
    Layout::from_size_align(size.checked_add(HDR)?, HDR).ok()
}

/// The block that starts `HDR` bytes before `p`, and the size C asked for.
///
/// # Safety
/// `p` must come from these functions and not have been freed.
unsafe fn header(p: *mut u8) -> (*mut u8, usize) {
    // SAFETY: as the caller promises, `p - HDR` is the start of our block, which holds the size.
    unsafe {
        let base = p.sub(HDR);
        (base, (base as *const usize).read())
    }
}

/// # Safety
/// `base` must be a fresh block of at least `HDR + size` bytes.
unsafe fn finish(base: *mut u8, size: usize) -> *mut u8 {
    if base.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: the block is ours and at least `HDR` bytes long.
    unsafe {
        (base as *mut usize).write(size);
        base.add(HDR)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn malloc(size: usize) -> *mut u8 {
    let Some(l) = layout(size) else {
        return ptr::null_mut();
    };
    // SAFETY: `l` has a non-zero size (it includes the header).
    unsafe { finish(alloc::alloc::alloc(l), size) }
}

#[unsafe(no_mangle)]
pub extern "C" fn calloc(n: usize, size: usize) -> *mut u8 {
    let Some(total) = n.checked_mul(size) else {
        return ptr::null_mut();
    };
    let Some(l) = layout(total) else {
        return ptr::null_mut();
    };
    // SAFETY: as in `malloc`; the block comes back zeroed.
    unsafe { finish(alloc::alloc::alloc_zeroed(l), total) }
}

/// # Safety
/// `p` is null or a live pointer from these functions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn free(p: *mut u8) {
    if p.is_null() {
        return;
    }
    // SAFETY: the caller passes a live block of ours; its layout is the one it was made with.
    unsafe {
        let (base, size) = header(p);
        alloc::alloc::dealloc(base, Layout::from_size_align_unchecked(size + HDR, HDR));
    }
}

/// # Safety
/// `p` is null or a live pointer from these functions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn realloc(p: *mut u8, size: usize) -> *mut u8 {
    if p.is_null() {
        return malloc(size);
    }
    let Some(new) = size.checked_add(HDR) else {
        return ptr::null_mut();
    };
    // SAFETY: a live block of ours; on success the old one is gone and the new one holds the data.
    unsafe {
        let (base, old) = header(p);
        let l = Layout::from_size_align_unchecked(old + HDR, HDR);
        finish(alloc::alloc::realloc(base, l, new), size)
    }
}

/// # Safety
/// `p` is null or a live pointer from these functions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc_usable_size(p: *mut u8) -> usize {
    if p.is_null() {
        return 0;
    }
    // SAFETY: a live block of ours.
    unsafe { header(p).1 }
}
