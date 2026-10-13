//! `kitsune_core` — pure, allocation-free OS logic shared by the kernel.
//!
//! Everything here is `no_std` in production but compiles against `std` under
//! `cargo test`, so the entire module tree is unit-testable on the host. The
//! kernel keeps only hardware glue (framebuffer, port I/O, PS/2, RTC, fonts);
//! all decision logic lives here and is covered by tests.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

// Most of the crate is fixed-buffer and allocation-free. The `web` rendering
// engine is the exception: an HTML/CSS box-model engine needs a dynamic DOM and
// display list, so it uses `alloc` (backed by the kernel's global allocator in
// production, and by `std` under tests).
extern crate alloc;

pub mod apps;
pub mod browsing;
pub mod format;
pub mod hw;
pub mod i18n;
pub mod network;
pub mod platform;
pub mod security;
pub mod storage;
pub mod system;
#[cfg(test)]
pub(crate) mod testutil;
pub mod ui;
pub mod windowing;

#[cfg(test)]
mod structure;

// Flat paths kept for the kernel, the fuzz targets and the benches: `kitsune_core::fs3` is
// `kitsune_core::storage::fs3`. Inside this crate, use the grouped path (`crate::storage::fs3`).
pub use apps::{activity, calc, editor2, fileman, shell, termui, viewer};
pub use browsing::{browser, redirect, shields, web};
pub use format::{
    base64, bmp, deflate, elf, gzip, image, inflate, md5, png, ppm, search, text as textenc,
    unixtime, webm, zip,
};
pub use network::{dns, icmp, lease, net, netstats, sntp, tlsverify, x509};
pub use platform::{appabi, appfs, appinstall, appmanifest, appnet, wasmsec};
pub use security::{account, password, perm, secret, session};
pub use storage::{blockcache, blockdev, fs, fs3, homes, secured, vfs};
pub use system::{
    clipboard, entropy, heap, input, keymap, klog, notify, paging, process, rng, schedule,
    settings, sysif, sysmon,
};
pub use ui::{
    anim, appart, brand, chrome, cursor, fontcache, gfx, glyph, iconart, layout, pointer, raster,
    style, textlayout, ttf, wallpaper, widgets,
};
pub use windowing::{compositor, launcher, snap, taskbar, window, winman, wm};

pub use apps::calc::Calc;
pub use browsing::browser::Browser;
pub use hw::rtc::Time;
pub use system::clipboard::Clipboard;
pub use system::keymap::{Key, Keymap};
pub use system::process::{ProcKind, ProcState, Process, ProcessTable};
pub use ui::anim::Anim;
pub use windowing::window::Rect;
