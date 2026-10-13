//! Network interface abstraction: the [`Nic`] trait every driver implements, the
//! [`Port`] that owns one and counts what crosses it, and [`probe`], which picks
//! the NIC at boot.
//!
//! Ownership is the whole point of this layer. There is exactly one `Port`; it is
//! moved into the network owner (`netd`, which runs on the `fetcher` thread), so
//! the compiler, not a state machine and a comment, guarantees that a single party
//! touches the hardware. Everything above the driver (the DHCP client, the ARP and
//! ping responder, smoltcp) talks to `&mut Port` and never names a concrete NIC.
//!
//! Selection at boot: virtio-net if the PCI bus has one, else the NE2000 ISA card
//! if one answers, else no network.

use crate::{ne2000, serial_println, virtio_net};
use alloc::boxed::Box;
use kitsune_core::net::Mac;
use kitsune_core::netstats::{NetStats, NicKind};

/// The one interface's counters; readable from any thread (`STATS.snapshot`).
pub static STATS: NetStats = NetStats::new();

/// The MAC of the probed NIC (all zero until [`probe`] found one), packed into
/// the low 48 bits so any thread can read it (the Settings "Rede" page does).
static MAC_BITS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// MAC address of the NIC in use, or all zeroes when there is none.
pub fn mac() -> [u8; 6] {
    let v = MAC_BITS.load(core::sync::atomic::Ordering::Relaxed);
    let b = v.to_be_bytes();
    [b[2], b[3], b[4], b[5], b[6], b[7]]
}

fn remember_mac(m: Mac) {
    let b = m.0;
    let v = u64::from_be_bytes([0, 0, b[0], b[1], b[2], b[3], b[4], b[5]]);
    MAC_BITS.store(v, core::sync::atomic::Ordering::Relaxed);
}

/// Why a frame was not sent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TxError {
    /// Empty, or longer than the 1514-byte Ethernet maximum.
    BadLength,
    /// The device had no room and did not make any within the driver's bound.
    Full,
}

/// A network driver: raw Ethernet frames in and out, polled (no interrupts).
pub trait Nic {
    /// Which driver this is (for statistics and logs).
    fn kind(&self) -> NicKind;
    /// The interface's hardware address.
    fn mac(&self) -> Mac;
    /// Transmit one frame (padded to the Ethernet minimum by the driver).
    fn send(&mut self, frame: &[u8]) -> Result<(), TxError>;
    /// Receive the next frame into `buf` (a frame longer than `buf` is
    /// truncated), returning its length, or `None` when nothing is waiting.
    /// Drivers account their own receive errors and drops in [`STATS`].
    fn poll(&mut self, buf: &mut [u8]) -> Option<usize>;
    /// Whether the link is up (always true for hardware that cannot tell).
    fn link_up(&self) -> bool;
}

/// The exclusive handle on the interface, with traffic accounting.
pub struct Port {
    nic: Box<dyn Nic>,
    mac: Mac,
}

impl Port {
    fn new(nic: Box<dyn Nic>) -> Port {
        let mac = nic.mac();
        STATS.set_nic(nic.kind());
        STATS.set_link(nic.link_up());
        Port { nic, mac }
    }

    pub fn mac(&self) -> Mac {
        self.mac
    }

    /// Which driver is behind the port.
    pub fn kind(&self) -> kitsune_core::netstats::NicKind {
        self.nic.kind()
    }

    /// Link state, also published to [`STATS`].
    pub fn link_up(&self) -> bool {
        let up = self.nic.link_up();
        STATS.set_link(up);
        up
    }

    /// Send a frame; returns whether the driver accepted it.
    pub fn send(&mut self, frame: &[u8]) -> bool {
        match self.nic.send(frame) {
            Ok(()) => {
                STATS.on_tx(frame.len().max(kitsune_core::net::MIN_TX_FRAME));
                true
            }
            Err(TxError::BadLength) => {
                STATS.on_tx_dropped();
                false
            }
            Err(TxError::Full) => {
                STATS.on_tx_error();
                false
            }
        }
    }

    /// Receive the next frame, if any.
    pub fn poll(&mut self, buf: &mut [u8]) -> Option<usize> {
        let n = self.nic.poll(buf)?;
        // Entropy: frame arrival time (the NIC is polled, so this is thread context; the call is the
        // same lock-free one the ISRs use: one TSC read + one atomic store).
        crate::rng::sample(crate::rng::NIC);
        STATS.on_rx(n);
        Some(n)
    }
}

/// Bring up the NIC: virtio-net, then NE2000, else `None` (the OS runs without
/// networking). `phys_offset` is the bootloader's physical-memory mapping (needed
/// for DMA).
pub fn probe(phys_offset: Option<u64>) -> Option<Port> {
    if let Some(off) = phys_offset
        && let Some(dev) = virtio_net::VirtioNet::probe(off)
    {
        serial_println!("net: using virtio-net, mac {}", MacFmt(dev.mac()));
        remember_mac(dev.mac());
        return Some(Port::new(Box::new(dev)));
    }
    if let Some(dev) = ne2000::Ne2000::probe() {
        serial_println!("net: using ne2000, mac {}", MacFmt(dev.mac()));
        remember_mac(dev.mac());
        return Some(Port::new(Box::new(dev)));
    }
    serial_println!("net: no network interface found");
    None
}

/// `52:54:00:12:34:56`.
pub struct MacFmt(pub Mac);

impl core::fmt::Display for MacFmt {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let m = self.0.0;
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            m[0], m[1], m[2], m[3], m[4], m[5]
        )
    }
}
