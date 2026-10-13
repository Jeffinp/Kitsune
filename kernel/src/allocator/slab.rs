//! Size classes for the small blocks of the kernel heap (`kitsune_core::heap::slab_class`).
//!
//! The browser's layout, the HTML parser and the TLS stack make tens of thousands of small
//! allocations per second. On the address-sorted linked list each of them walked the free blocks
//! (hundreds, once the heap is fragmented by picture-sized buffers) to allocate and again to free:
//! under emulation that was most of the CPU of a page with a playing video (`[trace] alloc ...
//! scanned/alloc`). Here a request of up to `SLAB_MAX` bytes takes a block from a **slab**: a chunk of
//! `SLAB_SIZE` bytes, aligned to its size and taken from the list, cut into blocks of one class. A
//! free block's owner is its address masked down to the slab; allocating and freeing are constant
//! time. A slab with free blocks is on its class's list; a slab that empties goes back to the
//! linked list (unless it is the only one of its class), so the memory is there again for big
//! buffers.

use super::LinkedListAllocator;
use core::alloc::Layout;
use core::ptr;
use kitsune_core::heap::{SLAB_CLASSES, SLAB_HEADER, SLAB_SIZE};

const CLASSES: usize = SLAB_CLASSES.len();

/// The header at the start of every slab (fits in `SLAB_HEADER` bytes).
#[repr(C)]
struct Slab {
    class: usize,
    /// Blocks handed out and not freed.
    used: usize,
    /// The first freed block (its first word points to the next).
    free: *mut u8,
    /// The next block never handed out, and the end of the last block.
    bump: usize,
    end: usize,
    /// The class's list of slabs with free blocks (null-terminated both ways).
    prev: *mut Slab,
    next: *mut Slab,
    listed: bool,
}

const _: () = assert!(core::mem::size_of::<Slab>() <= SLAB_HEADER);

/// The slabs of every class.
pub(super) struct Slabs {
    /// Per class, the first slab that has a free block.
    partial: [*mut Slab; CLASSES],
    /// Bytes of the slabs held, and of the blocks handed out (for the free-memory figure).
    pub(super) held: usize,
    pub(super) used: usize,
}

// SAFETY: the raw pointers point into the kernel heap, only touched under the allocator's lock.
unsafe impl Send for Slabs {}

impl Slabs {
    pub(super) const fn new() -> Slabs {
        Slabs {
            partial: [ptr::null_mut(); CLASSES],
            held: 0,
            used: 0,
        }
    }

    /// A block of class `c`, a new slab taken from `list` when the class has none free.
    pub(super) fn alloc(&mut self, c: usize, list: &mut LinkedListAllocator) -> *mut u8 {
        let mut s = self.partial[c];
        if s.is_null() {
            s = self.new_slab(c, list);
            if s.is_null() {
                return ptr::null_mut();
            }
        }
        let size = SLAB_CLASSES[c];
        // SAFETY: `s` is a slab on the class's list (made by `new_slab`, its header initialised and
        // inside the slab's memory, which the list allocator handed out and nothing else uses); the
        // lock gives exclusive access. A listed slab has a free block: a freed one or the bump.
        unsafe {
            let slab = &mut *s;
            let block = if !slab.free.is_null() {
                let b = slab.free;
                // The first word of a free block is the next free block.
                slab.free = b.cast::<*mut u8>().read();
                b
            } else {
                let b = slab.bump as *mut u8;
                slab.bump += size;
                b
            };
            slab.used += 1;
            self.used += size;
            if slab.free.is_null() && slab.bump + size > slab.end {
                self.unlink(s);
            }
            block
        }
    }

    /// Give back `block` of class `c` (allocated by [`Slabs::alloc`]).
    ///
    /// # Safety
    /// `block` must be a live block of class `c` from this allocator.
    pub(super) unsafe fn dealloc(
        &mut self,
        block: *mut u8,
        c: usize,
        list: &mut LinkedListAllocator,
    ) {
        let s = (block as usize & !(SLAB_SIZE - 1)) as *mut Slab;
        // SAFETY: by the contract `block` came from a slab, which starts at the block's address
        // rounded down to `SLAB_SIZE` (slabs are aligned to their size and blocks lie after the
        // header), so `s` is that slab's live header; the lock gives exclusive access; the block is
        // at least 16 bytes and 16-aligned, so its first word can hold the free-list link.
        unsafe {
            let slab = &mut *s;
            debug_assert_eq!(slab.class, c);
            block.cast::<*mut u8>().write(slab.free);
            slab.free = block;
            slab.used -= 1;
            self.used -= SLAB_CLASSES[c];
            if slab.used == 0 && !(self.partial[c] == s && slab.next.is_null()) {
                // Empty and not the class's last free slab: back to the list.
                if slab.listed {
                    self.unlink(s);
                }
                self.held -= SLAB_SIZE;
                list.dealloc(s.cast::<u8>(), slab_layout());
            } else if !slab.listed {
                self.link(s);
            }
        }
    }

    /// A fresh slab of class `c`, listed; null when the heap has no room.
    fn new_slab(&mut self, c: usize, list: &mut LinkedListAllocator) -> *mut Slab {
        let mem = list.alloc(slab_layout());
        if mem.is_null() {
            return ptr::null_mut();
        }
        let s = mem.cast::<Slab>();
        let start = mem as usize + SLAB_HEADER;
        let size = SLAB_CLASSES[c];
        // SAFETY: `mem` is a fresh, exclusively owned block of `SLAB_SIZE` bytes aligned to
        // `SLAB_SIZE` (the layout asks for it), so the header fits (checked at compile time) and is
        // aligned.
        unsafe {
            s.write(Slab {
                class: c,
                used: 0,
                free: ptr::null_mut(),
                bump: start,
                end: start + (SLAB_SIZE - SLAB_HEADER) / size * size,
                prev: ptr::null_mut(),
                next: ptr::null_mut(),
                listed: false,
            });
        }
        self.held += SLAB_SIZE;
        self.link(s);
        s
    }

    /// Put `s` at the head of its class's list.
    fn link(&mut self, s: *mut Slab) {
        // SAFETY: `s` and the list's slabs are live slab headers (see `alloc`); exclusive access.
        unsafe {
            let c = (*s).class;
            let head = self.partial[c];
            (*s).prev = ptr::null_mut();
            (*s).next = head;
            if !head.is_null() {
                (*head).prev = s;
            }
            (*s).listed = true;
            self.partial[c] = s;
        }
    }

    /// Take `s` off its class's list.
    fn unlink(&mut self, s: *mut Slab) {
        // SAFETY: as in `link`; `s` is on its class's list, so its neighbours are live headers.
        unsafe {
            let (p, n) = ((*s).prev, (*s).next);
            if p.is_null() {
                self.partial[(*s).class] = n;
            } else {
                (*p).next = n;
            }
            if !n.is_null() {
                (*n).prev = p;
            }
            (*s).prev = ptr::null_mut();
            (*s).next = ptr::null_mut();
            (*s).listed = false;
        }
    }
}

/// What a slab asks of the linked list.
fn slab_layout() -> Layout {
    // SAFETY: `SLAB_SIZE` is a non-zero power of two (a test in `kitsune_core::heap` checks it), so
    // it is a valid alignment, and as a size it does not overflow when rounded up to itself.
    unsafe { Layout::from_size_align_unchecked(SLAB_SIZE, SLAB_SIZE) }
}
