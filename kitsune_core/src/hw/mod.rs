//! Pure decision logic extracted from the kernel's hardware drivers.
//!
//! Each submodule owns the *decoding* half of a driver (bytes/registers in,
//! typed values out) so it can be unit-tested on the host. The kernel keeps only
//! the port I/O and MMIO glue and calls into these functions.

pub mod ac97;
pub mod ata;
pub mod audio;
pub mod pci;
pub mod perf;
pub mod ps2;
pub mod rtc;
pub mod virtio;
pub mod virtio_net;
