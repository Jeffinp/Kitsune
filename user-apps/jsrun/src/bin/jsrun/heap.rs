//! `malloc`, `calloc`, `realloc`, `free` and `malloc_usable_size` for the C side. Each block carries a
//! 16-byte header in front of what C sees (the size C asked for and the block's size class), so `free`
//! knows what to give back and the pointer stays 16-byte aligned.
//!
//! QuickJS makes millions of small allocations (atoms, shapes, objects, bytecode buffers while it
//! compiles). The SDK's allocator is a first-fit linked list: fine for a program with a few big
//! buffers, but every allocation walks the free blocks, and with QuickJS's churn that walk grew long
//! enough that compiling YouTube's 1.9 MB player took 62 s under QEMU (0.47 s natively with glibc's
//! malloc). So blocks up to [`SMALL_MAX`] bytes come from size classes of 16 bytes: a free list per
//! class (constant time), refilled from chunks of [`CHUNK`] bytes taken from the SDK's allocator; freed
//! small blocks stay in their class for the next allocation of that size. Bigger blocks go to the SDK's
//! allocator as before.

use core::alloc::Layout;
use core::cell::UnsafeCell;
use core::ptr;

extern crate alloc;

const HDR: usize = 16;
/// Size classes are multiples of this (header included).
const STEP: usize = 16;
/// The largest block (header included) served from a size class.
const SMALL_MAX: usize = 1024;
const CLASSES: usize = SMALL_MAX / STEP + 1;
/// What a size class takes from the SDK's allocator when it has nothing free.
const CHUNK: usize = 256 * 1024;

struct Pools {
    /// The first free block of each class (its first word points to the next).
    free: [*mut u8; CLASSES],
    /// The rest of the current chunk, carved in order.
    bump: *mut u8,
    bump_end: *mut u8,
}

struct Shared(UnsafeCell<Pools>);
// SAFETY: the process has a single thread (QuickJS runs on it alone) and none of these functions
// re-enter one another.
unsafe impl Sync for Shared {}

static POOLS: Shared = Shared(UnsafeCell::new(Pools {
    free: [ptr::null_mut(); CLASSES],
    bump: ptr::null_mut(),
    bump_end: ptr::null_mut(),
}));

/// The whole block for `size` bytes of C data, rounded to [`STEP`]; `None` when absurdly large.
fn total(size: usize) -> Option<usize> {
    size.checked_add(HDR + STEP - 1)
        .filter(|&t| t <= isize::MAX as usize / 2)
        .map(|t| t & !(STEP - 1))
}

/// The layout of a big block of `total` bytes (a value from [`total`]).
fn big_layout(total: usize) -> Layout {
    // SAFETY: `total` comes from `total()`: a non-zero multiple of 16 no larger than isize::MAX / 2,
    // and 16 is a power of two.
    unsafe { Layout::from_size_align_unchecked(total, HDR) }
}

/// A block of class `cls` (`cls * STEP` bytes), or null when out of memory.
///
/// # Safety
/// Single-threaded use only (see [`Shared`]).
unsafe fn small_alloc(cls: usize) -> *mut u8 {
    // SAFETY: single thread, no re-entry (the SDK allocator does not call back here).
    let p = unsafe { &mut *POOLS.0.get() };
    let head = p.free[cls];
    if !head.is_null() {
        // SAFETY: `head` is a free block of this class; its first word holds the next one.
        p.free[cls] = unsafe { (head as *const *mut u8).read() };
        return head;
    }
    let need = cls * STEP;
    if (p.bump_end as usize).saturating_sub(p.bump as usize) < need {
        // SAFETY: CHUNK is a non-zero multiple of 16.
        let c = unsafe { alloc::alloc::alloc(Layout::from_size_align_unchecked(CHUNK, HDR)) };
        if c.is_null() {
            return ptr::null_mut();
        }
        p.bump = c;
        // SAFETY: `c` is the start of CHUNK bytes.
        p.bump_end = unsafe { c.add(CHUNK) };
    }
    let b = p.bump;
    // SAFETY: at least `need` bytes are left before `bump_end` (checked above).
    p.bump = unsafe { b.add(need) };
    b
}

/// Write the header of `base` and return what C sees.
///
/// # Safety
/// `base` is null or a fresh block of at least `HDR + size` bytes.
unsafe fn finish(base: *mut u8, size: usize, cls: usize) -> *mut u8 {
    if base.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: the block is ours and at least `HDR` bytes long, 16-byte aligned.
    unsafe {
        (base as *mut usize).write(size);
        (base as *mut usize).add(1).write(cls);
        base.add(HDR)
    }
}

/// The block that starts `HDR` bytes before `p`, the size C asked for, and its class (0: big).
///
/// # Safety
/// `p` must come from these functions and not have been freed.
unsafe fn header(p: *mut u8) -> (*mut u8, usize, usize) {
    // SAFETY: as the caller promises, `p - HDR` is the start of our block, which holds the header.
    unsafe {
        let base = p.sub(HDR);
        (
            base,
            (base as *const usize).read(),
            (base as *const usize).add(1).read(),
        )
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn malloc(size: usize) -> *mut u8 {
    let Some(t) = total(size) else {
        return ptr::null_mut();
    };
    if t <= SMALL_MAX {
        let cls = t / STEP;
        // SAFETY: single thread; the block holds `t` >= HDR + size bytes.
        return unsafe { finish(small_alloc(cls), size, cls) };
    }
    // SAFETY: `big_layout(t)` has a non-zero size; the block holds `t` bytes.
    unsafe { finish(alloc::alloc::alloc(big_layout(t)), size, 0) }
}

#[unsafe(no_mangle)]
pub extern "C" fn calloc(n: usize, size: usize) -> *mut u8 {
    let Some(bytes) = n.checked_mul(size) else {
        return ptr::null_mut();
    };
    let Some(t) = total(bytes) else {
        return ptr::null_mut();
    };
    if t > SMALL_MAX {
        // SAFETY: as in `malloc`; the block comes back zeroed.
        return unsafe { finish(alloc::alloc::alloc_zeroed(big_layout(t)), bytes, 0) };
    }
    let p = malloc(bytes);
    if !p.is_null() {
        // SAFETY: `p` holds at least `bytes` bytes (a block of a size class may be a reused one).
        unsafe { ptr::write_bytes(p, 0, bytes) };
    }
    p
}

/// # Safety
/// `p` is null or a live pointer from these functions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn free(p: *mut u8) {
    if p.is_null() {
        return;
    }
    // SAFETY: the caller passes a live block of ours.
    unsafe {
        let (base, size, cls) = header(p);
        if cls == 0 {
            let t = total(size).unwrap_or(HDR); // a live block's size always passed `total`
            alloc::alloc::dealloc(base, big_layout(t));
            return;
        }
        let pools = &mut *POOLS.0.get();
        (base as *mut *mut u8).write(pools.free[cls]);
        pools.free[cls] = base;
    }
}

/// # Safety
/// `p` is null or a live pointer from these functions.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn realloc(p: *mut u8, size: usize) -> *mut u8 {
    if p.is_null() {
        return malloc(size);
    }
    let Some(t) = total(size) else {
        return ptr::null_mut();
    };
    // SAFETY: a live block of ours; on success the old one is gone and the new one holds the data.
    unsafe {
        let (base, old, cls) = header(p);
        if cls != 0 && t <= cls * STEP {
            // Still fits its block.
            (base as *mut usize).write(size);
            return p;
        }
        if cls == 0 && t > SMALL_MAX {
            let old_t = total(old).unwrap_or(HDR); // as in `free`
            return finish(alloc::alloc::realloc(base, big_layout(old_t), t), size, 0);
        }
        let n = malloc(size);
        if n.is_null() {
            return ptr::null_mut();
        }
        ptr::copy_nonoverlapping(p, n, old.min(size));
        free(p);
        n
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
