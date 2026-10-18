//! Ring bookkeeping. The memory is the kernel's; these types decide which slot to write next, which
//! cycle bit to stamp, and when the ring wraps, so the wrap logic is tested without a controller.
//!
//! The cycle bit is how producer and consumer agree on who owns a slot without sharing an index: the
//! producer stamps its current cycle state into each TRB it writes; the consumer reads TRBs whose cycle
//! bit equals its own state and flips that state each time it wraps. Both start at 1 (rings are zeroed).

use super::trb::{TRB_BYTES, Trb};

/// A command or transfer ring the driver writes. The last slot is a Link TRB back to the start.
///
/// The ring does not know how far the controller has read, so the caller must not have more TRBs
/// outstanding than [`ProducerRing::capacity`]. Commands and control transfers are issued one at a time,
/// which keeps that trivially true.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProducerRing {
    base: u64,
    size: u16,
    enqueue: u16,
    cycle: bool,
}

/// What [`ProducerRing::push`] decided: write `trb` at slot `index`; if `link` is set, then write that
/// Link TRB at its slot too (the ring just reached its end and wraps).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pushed {
    pub index: u16,
    pub trb: Trb,
    pub link: Option<(u16, Trb)>,
}

impl ProducerRing {
    /// A ring of `size` TRBs (the last one is the link) at physical address `base`. `None` when `base`
    /// is not 16-byte aligned or `size` leaves no room for a data slot.
    pub const fn new(base: u64, size: u16) -> Option<Self> {
        if base & 0xF != 0 || size < 2 {
            return None;
        }
        Some(Self {
            base,
            size,
            enqueue: 0,
            cycle: true,
        })
    }

    /// TRBs that can be outstanding at once.
    pub const fn capacity(&self) -> u16 {
        self.size - 1
    }

    /// Bytes of memory the ring occupies.
    pub const fn bytes(&self) -> usize {
        self.size as usize * TRB_BYTES
    }

    /// Where the next TRB goes, with the current cycle state in bit 0: the format of the command ring
    /// pointer (`CRCR`) and of a transfer ring's dequeue pointer in an endpoint context.
    pub const fn pointer(&self) -> u64 {
        (self.base + self.enqueue as u64 * TRB_BYTES as u64) | self.cycle as u64
    }

    /// Physical address of slot `index`.
    pub const fn address_of(&self, index: u16) -> u64 {
        self.base + index as u64 * TRB_BYTES as u64
    }

    /// Takes the next slot for `trb`, stamping the cycle bit. After the last data slot it also returns
    /// the Link TRB (carrying the cycle of the lap that just ended) and starts a new lap.
    pub fn push(&mut self, trb: Trb) -> Pushed {
        let index = self.enqueue;
        let stamped = trb.with_cycle(self.cycle);
        self.enqueue += 1;
        let link = if self.enqueue == self.size - 1 {
            let last = self.size - 1;
            let link = Trb::link(self.base, true).with_cycle(self.cycle);
            self.enqueue = 0;
            self.cycle = !self.cycle;
            Some((last, link))
        } else {
            None
        };
        Pushed {
            index,
            trb: stamped,
            link,
        }
    }
}

/// The event ring the controller writes and the driver reads (a single segment).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventRing {
    base: u64,
    size: u16,
    dequeue: u16,
    cycle: bool,
}

impl EventRing {
    /// A segment of `size` TRBs at `base`; `None` if `base` is misaligned or `size` is zero.
    pub const fn new(base: u64, size: u16) -> Option<Self> {
        if base & 0xF != 0 || size == 0 {
            return None;
        }
        Some(Self {
            base,
            size,
            dequeue: 0,
            cycle: true,
        })
    }

    pub const fn size(&self) -> u16 {
        self.size
    }

    /// Slot the next event will appear in.
    pub const fn dequeue_index(&self) -> u16 {
        self.dequeue
    }

    /// The TRB at the dequeue slot is a new event: its cycle bit matches ours.
    pub const fn is_new(&self, trb: &Trb) -> bool {
        trb.cycle() == self.cycle
    }

    /// Consumes the event at the dequeue slot.
    pub fn advance(&mut self) {
        self.dequeue += 1;
        if self.dequeue == self.size {
            self.dequeue = 0;
            self.cycle = !self.cycle;
        }
    }

    /// Value for the event ring dequeue pointer register (`ERDP`): the address of the next slot to read.
    /// The caller adds [`super::ERDP_EHB`] to clear the busy flag.
    pub const fn pointer(&self) -> u64 {
        self.base + self.dequeue as u64 * TRB_BYTES as u64
    }
}
