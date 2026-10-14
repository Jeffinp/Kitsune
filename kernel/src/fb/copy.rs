//! Copying pixel rows without `rep movs`.
//!
//! The kernel's `memcpy`/`memmove` (from `compiler_builtins`) copy with `rep movsq`. On real
//! hardware and under KVM/WHPX that is as fast as anything, but QEMU's TCG runs a `rep` string
//! instruction one element at a time, each a trip through its dispatcher: copying a picture row by
//! row that way costs several times a plain loop of 8-byte moves (the same finding as the
//! `memcpy` of `mediadec`, `docs/design/midia.md`). The rows of a video picture and the uploads of
//! the damage to the framebuffer go through [`copy_bytes`]: 32 bytes per step in general purpose
//! registers (no SSE: the kernel is built without it, and an interrupt handler may run anywhere),
//! the tail (under 32 bytes) by the ordinary copy.

/// Use the loop below (else the ordinary copy, to measure the difference).
const LOOP: bool = true;

/// Copy `src` into `dst` (the same length; they must not overlap, which `&mut`/`&` guarantee).
#[inline]
pub fn copy_bytes(dst: &mut [u8], src: &[u8]) {
    let n = dst.len().min(src.len());
    let blocks = if LOOP { n / 32 } else { 0 };
    if blocks > 0 {
        // SAFETY: `src` and `dst` are valid for `n >= blocks * 32` bytes (slice lengths) and do not
        // overlap (one is borrowed mutably, the other shared). The loop reads and writes exactly
        // `blocks * 32` bytes from each start with unaligned 8-byte moves (legal on x86), uses only
        // the named scratch registers and advances the pointers it was given; no stack, flags
        // clobbered (not preserved).
        unsafe {
            core::arch::asm!(
                "2:",
                "mov {a}, [{s}]",
                "mov {b}, [{s} + 8]",
                "mov [{d}], {a}",
                "mov [{d} + 8], {b}",
                "mov {a}, [{s} + 16]",
                "mov {b}, [{s} + 24]",
                "mov [{d} + 16], {a}",
                "mov [{d} + 24], {b}",
                "add {s}, 32",
                "add {d}, 32",
                "dec {n}",
                "jnz 2b",
                s = inout(reg) src.as_ptr() => _,
                d = inout(reg) dst.as_mut_ptr() => _,
                n = inout(reg) blocks => _,
                a = out(reg) _,
                b = out(reg) _,
                options(nostack),
            );
        }
    }
    let done = blocks * 32;
    dst[done..n].copy_from_slice(&src[done..n]);
}
