//! The global allocator of a program that uses `alloc` (feature `heap`): `linked_list_allocator` over
//! pages the kernel grants with `brk`, one megabyte at a time, up to the process's limit.

use crate::{HEAP_BASE, brk};
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr::{self, NonNull};
use linked_list_allocator::Heap;

const MIB: u64 = 1 << 20;

struct HeapState {
    heap: Heap,
    /// Current end of the heap as granted by `brk` (0 = not initialised).
    end: u64,
}

struct Allocator(UnsafeCell<HeapState>);

// SAFETY: the process has a single thread.
unsafe impl Sync for Allocator {}

#[global_allocator]
static ALLOC: Allocator = Allocator(UnsafeCell::new(HeapState {
    heap: Heap::empty(),
    end: 0,
}));

impl HeapState {
    fn init(&mut self) {
        let end = brk(HEAP_BASE + MIB);
        if end >= HEAP_BASE + MIB {
            // SAFETY: [HEAP_BASE, end) was just granted by the kernel and nothing else uses it.
            unsafe {
                self.heap
                    .init(HEAP_BASE as *mut u8, (end - HEAP_BASE) as usize)
            };
            self.end = end;
        } else {
            self.end = u64::MAX; // no heap at all: every allocation fails
        }
    }

    fn grow(&mut self, need: usize) -> bool {
        if self.end == u64::MAX {
            return false;
        }
        let want = (need as u64).saturating_add(MIB - 1) / MIB * MIB;
        let target = self.end.saturating_add(want.max(MIB));
        let got = brk(target);
        if got <= self.end {
            return false;
        }
        let by = (got - self.end) as usize;
        self.end = got;
        // SAFETY: the new range directly follows the heap and is now mapped.
        unsafe { self.heap.extend(by) };
        true
    }
}

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: single thread, no reentrancy (brk does not allocate).
        let st = unsafe { &mut *self.0.get() };
        if st.end == 0 {
            st.init();
        }
        for _ in 0..2 {
            if let Ok(p) = st.heap.allocate_first_fit(layout) {
                return p.as_ptr();
            }
            // Room for the block plus the allocator's bookkeeping and alignment slack.
            if !st.grow(
                layout
                    .size()
                    .saturating_add(layout.align())
                    .saturating_add(64),
            ) {
                break;
            }
        }
        match st.heap.allocate_first_fit(layout) {
            Ok(p) => p.as_ptr(),
            Err(()) => ptr::null_mut(),
        }
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        // SAFETY: single thread.
        let st = unsafe { &mut *self.0.get() };
        if let Some(p) = NonNull::new(p) {
            // SAFETY: `p` came from `alloc` with this layout.
            unsafe { st.heap.deallocate(p, layout) };
        }
    }
}
