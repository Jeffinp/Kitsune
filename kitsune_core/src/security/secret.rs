//! [`Secret`]: a typed password that is wiped from memory when it is cleared or dropped.
//!
//! A `String` that held a password leaves its bytes on the heap after `clear()` or `drop`, and a
//! growing `String` leaves old copies behind when it reallocates. `Secret` allocates its whole
//! capacity once, refuses input beyond it (so it never reallocates), overwrites the bytes with
//! zeros when cleared or dropped, and does not print its content. It cannot stop a copy the caller
//! makes with [`Secret::as_str`], nor the stack frames of hashing; it removes the long-lived copy.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// Longest password typed (bytes).
pub const MAX_SECRET: usize = 64;

/// Zero `buf` in a way the optimiser may not drop (the barrier makes the stores observable).
fn wipe(buf: &mut Vec<u8>) {
    buf.iter_mut().for_each(|b| *b = 0);
    core::hint::black_box(&mut *buf);
    buf.clear();
}

pub struct Secret {
    buf: Vec<u8>,
    cap: usize,
}

impl Secret {
    /// An empty secret that can hold `cap` bytes (`cap` is the most it will ever allocate).
    pub fn with_capacity(cap: usize) -> Secret {
        Secret {
            buf: Vec::with_capacity(cap),
            cap,
        }
    }

    pub fn new() -> Secret {
        Secret::with_capacity(MAX_SECRET)
    }

    /// Append `c`; `false` (and nothing appended) when it would not fit.
    pub fn push(&mut self, c: char) -> bool {
        let mut b = [0u8; 4];
        let s = c.encode_utf8(&mut b);
        if self.buf.len() + s.len() > self.cap {
            return false;
        }
        self.buf.extend_from_slice(s.as_bytes());
        wipe_array(&mut b);
        true
    }

    /// Remove the last character. `false` when empty.
    pub fn pop(&mut self) -> bool {
        let Some(s) = core::str::from_utf8(&self.buf).ok() else {
            return false;
        };
        let Some(c) = s.chars().next_back() else {
            return false;
        };
        let keep = self.buf.len() - c.len_utf8();
        for b in &mut self.buf[keep..] {
            *b = 0;
        }
        core::hint::black_box(&mut self.buf);
        self.buf.truncate(keep);
        true
    }

    /// Wipe and empty.
    pub fn clear(&mut self) {
        wipe(&mut self.buf);
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf).unwrap_or("")
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// How many characters it holds (for drawing bullets).
    pub fn chars(&self) -> usize {
        self.as_str().chars().count()
    }

    /// A second secret with the same content (its own wiped buffer).
    pub fn duplicate(&self) -> Secret {
        let mut s = Secret::with_capacity(self.cap);
        s.buf.extend_from_slice(&self.buf);
        s
    }

    /// The content as an owned string, for APIs that want one. The copy is the caller's to drop;
    /// prefer [`Secret::as_str`].
    pub fn to_plain(&self) -> String {
        String::from(self.as_str())
    }
}

fn wipe_array(b: &mut [u8; 4]) {
    *b = [0; 4];
    core::hint::black_box(&mut *b);
}

impl Default for Secret {
    fn default() -> Self {
        Secret::new()
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        wipe(&mut self.buf);
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Secret({} chars)", self.chars())
    }
}

#[cfg(test)]
mod tests;
