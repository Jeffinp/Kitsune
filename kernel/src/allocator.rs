//! Kernel heap: a linked-list free allocator behind a spin lock, exposed as the
//! global allocator so `alloc` (Vec/String/Box) works. The fiddly alignment and
//! region-fit math is in `kitsune_core::heap` (unit-tested); this file is the
//! thin `unsafe` glue that threads free nodes through the heap memory. Small
//! blocks (up to `kitsune_core::heap::SLAB_MAX` bytes) come from size classes
//! (`slab.rs`) carved out of the same list.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::mem;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};
use kitsune_core::heap::{adjust_request, fit_region_split, regions_adjacent, slab_class};

mod slab;

// ---- minimal spin lock ----

pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// Safe: access is serialized by the lock; the kernel is single-core and our
// ISRs never allocate, so there is no re-entrancy.
// SAFETY: `data` is only reachable through a `SpinGuard`, which exists only while `locked`
// is held (CAS Acquire / store Release). `lock` also clears IF, and the kernel is
// single-core with ISRs that never allocate, so there is no re-entrancy.
unsafe impl<T: Send> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> SpinGuard<'_, T> {
        // Disable interrupts while the lock is held so a timer preemption can't
        // switch to a thread that then deadlocks waiting on the same lock.
        let ints_enabled = x86_64::instructions::interrupts::are_enabled();
        x86_64::instructions::interrupts::disable();
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        SpinGuard {
            lock: self,
            ints_enabled,
        }
    }
}

pub struct SpinGuard<'a, T> {
    lock: &'a SpinLock<T>,
    ints_enabled: bool,
}

impl<T> core::ops::Deref for SpinGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the guard proves `locked` is held (and IF was cleared by `lock`), so no other
        // reference to `data` exists; the pointer comes from a live `UnsafeCell`.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> core::ops::DerefMut for SpinGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as in `deref`; `&mut self` makes this the only access through the guard.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SpinGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        if self.ints_enabled {
            x86_64::instructions::interrupts::enable();
        }
    }
}

// ---- linked-list allocator ----

struct FreeNode {
    size: usize,
    next: Option<&'static mut FreeNode>,
}

impl FreeNode {
    const fn new(size: usize) -> Self {
        FreeNode { size, next: None }
    }
    fn start_addr(&self) -> usize {
        self as *const Self as usize
    }
    fn end_addr(&self) -> usize {
        self.start_addr() + self.size
    }
}

fn node_size() -> usize {
    mem::size_of::<FreeNode>()
}
fn node_align() -> usize {
    mem::align_of::<FreeNode>()
}

pub struct LinkedListAllocator {
    head: FreeNode,
}

impl LinkedListAllocator {
    pub const fn new() -> Self {
        Self {
            head: FreeNode::new(0),
        }
    }

    /// # Safety
    /// `start..start+size` must be valid, unused, writable memory that lives for
    /// the rest of the program.
    ///
    /// The start must also be 8-byte aligned and `size >= 16` (a free node); that is only
    /// `debug_assert!`ed in `add_free_region`.
    pub unsafe fn init(&mut self, start: usize, size: usize) {
        // SAFETY: forwards this fn's contract (valid, unused, writable, 'static memory), plus the
        // alignment/size note above.
        unsafe { self.add_free_region(start, size) };
    }

    /// Insert a freed region into the address-sorted free list and coalesce it
    /// with any physically adjacent neighbours. Without coalescing, repeated
    /// alloc/free of mixed sizes would fragment the heap permanently — large
    /// requests would fail even with plenty of (scattered) free memory.
    ///
    /// # Safety
    /// `addr..addr + size` must be free memory owned by the allocator (from `init`, the unused
    /// tail of a region just unlinked by `find_region`, or a block being freed), 8-aligned,
    /// `size >= 16`, not on the list, and the caller must have exclusive access to `self`.
    unsafe fn add_free_region(&mut self, addr: usize, size: usize) {
        debug_assert_eq!(kitsune_core::heap::align_up(addr, node_align()), addr);
        debug_assert!(size >= node_size());

        // SAFETY: by this fn's contract `addr..addr+size` is free, aligned and unlinked, so the node
        // write is in bounds; `prev` is `self.head` or a node already on the list (free memory the
        // allocator owns). `&mut self` (behind the SpinLock) excludes concurrent access.
        // NOTE: the `&'static mut` links vs the raw `node_ptr`/`prev` re-borrows are not Miri-checked.
        unsafe {
            let node_ptr = addr as *mut FreeNode;
            node_ptr.write(FreeNode::new(size));

            // Walk the sorted list to the last node whose start is below `addr`
            // (the head is a sentinel with start = &head, size 0, so it never
            // coalesces with a real region).
            let mut prev: *mut FreeNode = &mut self.head;
            while let Some(next) = (*prev).next.as_ref() {
                if next.start_addr() < addr {
                    prev = (*prev).next.as_deref_mut().unwrap() as *mut FreeNode;
                } else {
                    break;
                }
            }

            // Link the new node in between `prev` and `prev.next`.
            (*node_ptr).next = (*prev).next.take();
            (*prev).next = Some(&mut *node_ptr);

            // Merge forward (node + successor), then backward (prev + node).
            // Order matters: collapsing the successor first closes a three-way gap.
            Self::merge_with_next(node_ptr);
            Self::merge_with_next(prev);
        }
    }

    /// If `node` ends exactly where its successor begins, absorb the successor.
    ///
    /// # Safety
    /// `node` must point to a valid `FreeNode` on the list, with no other live reference to it.
    unsafe fn merge_with_next(node: *mut FreeNode) {
        // SAFETY: `node` is `prev` or `node_ptr` from `add_free_region`: a valid list node, and the
        // caller's `&mut self` rules out other live references to it.
        let n = unsafe { &mut *node };
        let adjacent = match n.next.as_deref() {
            Some(next) => regions_adjacent(n.start_addr(), n.size, next.start_addr()),
            None => false,
        };
        if adjacent {
            let next = n.next.take().unwrap();
            n.size += next.size;
            n.next = next.next.take();
        }
    }

    /// Remove and return the first region that fits, plus the chosen start.
    fn find_region(&mut self, size: usize, align: usize) -> Option<(&'static mut FreeNode, usize)> {
        let mut current = &mut self.head;
        let mut scanned = 0u64;
        while let Some(ref mut region) = current.next {
            scanned += 1;
            if let Some(fit) =
                fit_region_split(region.start_addr(), region.size, size, align, node_size())
            {
                let alloc_start = fit.alloc_start;
                let next = region.next.take();
                let region = current.next.take().unwrap();
                current.next = next;
                if crate::trace::ON {
                    crate::trace::ALLOC_SCANNED
                        .fetch_add(scanned, core::sync::atomic::Ordering::Relaxed);
                }
                return Some((region, alloc_start));
            }
            current = current.next.as_mut().unwrap();
        }
        if crate::trace::ON {
            crate::trace::ALLOC_SCANNED.fetch_add(scanned, core::sync::atomic::Ordering::Relaxed);
        }
        None
    }

    fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let (size, align) =
            adjust_request(layout.size(), layout.align(), node_size(), node_align());
        match self.find_region(size, align) {
            Some((region, alloc_start)) => {
                let alloc_end = alloc_start + size;
                // Read everything we need from the node before the writes below can overwrite it.
                let region_start = region.start_addr();
                let excess = region.end_addr() - alloc_end;
                let front = alloc_start - region_start;
                if front > 0 {
                    // SAFETY: `find_region` unlinked the whole region, so `region_start..alloc_start` is free
                    // and unaliased; both ends are 8-aligned (`alloc_start` is aligned to `align >= 8`, the
                    // region start is a node address) and `fit_region_split` guarantees `front >= node_size`
                    // when `front > 0`. This returns the alignment padding that used to leak.
                    unsafe { self.add_free_region(region_start, front) };
                }
                if excess > 0 {
                    // SAFETY: `find_region` unlinked the whole region, so `alloc_end..region_end` is free and
                    // unaliased; it is 8-aligned (start and size are multiples of node_align) and `fit_region_split`
                    // guarantees `excess >= node_size` when `excess > 0`.
                    unsafe { self.add_free_region(alloc_end, excess) };
                }
                alloc_start as *mut u8
            }
            None => ptr::null_mut(),
        }
    }

    fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        let (size, _align) =
            adjust_request(layout.size(), layout.align(), node_size(), node_align());
        // SAFETY: private, only called from `GlobalAlloc::dealloc`, whose contract says `ptr`/`layout`
        // match a live `alloc`, so `ptr..ptr+size` (same `adjust_request` size) is unused and 8-aligned.
        // NOTE: not validated here; a wrong layout would corrupt the free list (std contract).
        unsafe { self.add_free_region(ptr as usize, size) };
    }

    /// Total bytes currently on the free list (sum of all free regions).
    fn free_bytes(&self) -> usize {
        let mut total = 0;
        let mut cur = self.head.next.as_deref();
        while let Some(node) = cur {
            total += node.size;
            cur = node.next.as_deref();
        }
        total
    }
}

impl Default for LinkedListAllocator {
    fn default() -> Self {
        Self::new()
    }
}

// ---- global allocator ----

/// The linked list (big blocks and the slabs themselves) and the size classes (small blocks).
pub struct Heap {
    list: LinkedListAllocator,
    slabs: slab::Slabs,
}

impl Heap {
    fn alloc(&mut self, layout: Layout) -> *mut u8 {
        match slab_class(layout.size(), layout.align()) {
            Some(c) => self.slabs.alloc(c, &mut self.list),
            None => self.list.alloc(layout),
        }
    }

    /// # Safety
    /// `ptr` and `layout` must be a live block of this heap and the layout it was allocated with.
    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        match slab_class(layout.size(), layout.align()) {
            // SAFETY: the same layout picked the same class when the block was allocated (the
            // caller's contract), so it is a live block of that class.
            Some(c) => unsafe { self.slabs.dealloc(ptr, c, &mut self.list) },
            None => self.list.dealloc(ptr, layout),
        }
    }
}

pub struct LockedHeap(SpinLock<Heap>);

impl LockedHeap {
    pub const fn new() -> Self {
        Self(SpinLock::new(Heap {
            list: LinkedListAllocator::new(),
            slabs: slab::Slabs::new(),
        }))
    }

    /// # Safety
    /// See [`LinkedListAllocator::init`].
    pub unsafe fn init(&self, start: usize, size: usize) {
        // SAFETY: the caller's contract (see `LinkedListAllocator::init`) is forwarded unchanged;
        // the lock gives exclusive access to the allocator.
        unsafe { self.0.lock().list.init(start, size) };
    }

    /// Bytes currently free on the heap (for the perf HUD): the list's, and the free blocks of
    /// the slabs.
    pub fn free_bytes(&self) -> usize {
        let h = self.0.lock();
        h.list.free_bytes() + h.slabs.held.saturating_sub(h.slabs.used)
    }
}

// SAFETY: `alloc` returns null or a block of at least `layout.size()` bytes aligned to at
// least `layout.align()` (adjust_request/fit_region), carved from a region unlinked from the
// free list, so live blocks never overlap; `dealloc` relinks it. State is behind the SpinLock.
// NOTE: `dealloc` does not validate `layout` (the standard GlobalAlloc contract).
unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if crate::trace::ON {
            use core::sync::atomic::Ordering::Relaxed;
            let t0 = crate::io::rdtsc();
            let p = self.0.lock().alloc(layout);
            let d = crate::io::rdtsc().wrapping_sub(t0);
            crate::trace::ALLOC_N.fetch_add(1, Relaxed);
            crate::trace::ALLOC_BYTES.fetch_add(layout.size() as u64, Relaxed);
            crate::trace::ALLOC_CYC.fetch_add(d, Relaxed);
            crate::trace::max_to(&crate::trace::ALLOC_MAX, d);
            return p;
        }
        self.0.lock().alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if crate::trace::ON {
            use core::sync::atomic::Ordering::Relaxed;
            let t0 = crate::io::rdtsc();
            // SAFETY: `GlobalAlloc::dealloc`'s contract: a live block of this allocator and its layout.
            unsafe { self.0.lock().dealloc(ptr, layout) };
            crate::trace::FREE_N.fetch_add(1, Relaxed);
            crate::trace::FREE_CYC.fetch_add(crate::io::rdtsc().wrapping_sub(t0), Relaxed);
            return;
        }
        // SAFETY: `GlobalAlloc::dealloc`'s contract: a live block of this allocator and its layout.
        unsafe { self.0.lock().dealloc(ptr, layout) };
    }
}
