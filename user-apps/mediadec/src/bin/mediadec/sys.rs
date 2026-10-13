//! What the C side asks of the system (`kmd_*`, declared in `csrc/libc/libc.c`): output and the end
//! of the process. Only integers and pointers cross: this Rust is soft-float, the C uses SSE.

use kitsune_user as sdk;

/// Write `len` bytes at `buf` to fd 1 or 2 (both are the process output). Returns bytes taken or
/// `-errno`.
///
/// # Safety
/// `buf` must be valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kmd_write(fd: i32, buf: *const u8, len: usize) -> i64 {
    if len == 0 {
        return 0;
    }
    // SAFETY: the caller promises `buf` holds `len` bytes.
    let data = unsafe { core::slice::from_raw_parts(buf, len) };
    sdk::write(if fd == 2 { 2 } else { 1 }, &data[..len.min(4096)])
}

/// End the process.
#[unsafe(no_mangle)]
pub extern "C" fn kmd_exit(code: i32) -> ! {
    sdk::exit(code)
}
