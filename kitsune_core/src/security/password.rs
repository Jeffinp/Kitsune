//! Password storage: PBKDF2-HMAC-SHA-256 with a per-user salt, in a self-describing string.
//!
//! Stored form: `$kpw1$<iterations>$<salt hex>$<hash hex>`. The iteration count travels with the
//! hash, so it can be raised later and old hashes still verify (and report [`needs_rehash`]).
//! A password is never kept, logged or compared directly: [`verify`] recomputes the hash and
//! compares in constant time. The salt must come from the system's entropy
//! (`kitsune_core::entropy`); this module draws no randomness itself.

use alloc::string::String;
use alloc::vec::Vec;
use sha2::{Digest, Sha256};

/// Iterations for new hashes. Software SHA-256 on the kernel target is slow, so this is a
/// compromise (a login costs a fraction of a second); raise it with the hardware.
pub const DEFAULT_ITERATIONS: u32 = 20_000;
/// The least a stored hash may claim (an attacker-edited database cannot make `verify` free).
pub const MIN_ITERATIONS: u32 = 1_000;
/// The most `verify` will spend on one hash (an edited database cannot make it hang).
pub const MAX_ITERATIONS: u32 = 100_000;
/// Salt length for new hashes, in bytes.
pub const SALT_LEN: usize = 16;
/// Longest password accepted, in bytes.
pub const MAX_PASSWORD: usize = 256;

const TAG: &str = "$kpw1$";
const OUT: usize = 32;
const BLOCK: usize = 64;

/// HMAC-SHA-256 (RFC 2104).
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..OUT].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(data);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner);
    outer.finalize().into()
}

/// PBKDF2-HMAC-SHA-256 (RFC 8018), `OUT` bytes of derived key.
///
/// The key block is the same in every iteration, so the hash states after absorbing `ipad` and
/// `opad` are computed once and cloned: each iteration then costs two compressions instead of four.
pub fn pbkdf2(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut k = [0u8; BLOCK];
    if password.len() > BLOCK {
        k[..OUT].copy_from_slice(&Sha256::digest(password));
    } else {
        k[..password.len()].copy_from_slice(password);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner0 = Sha256::new();
    inner0.update(ipad);
    let mut outer0 = Sha256::new();
    outer0.update(opad);
    let hmac = |data: &[u8]| -> [u8; 32] {
        let mut inner = inner0.clone();
        inner.update(data);
        let mut outer = outer0.clone();
        outer.update(inner.finalize());
        outer.finalize().into()
    };
    // One block is enough (dkLen = hLen): U1 = HMAC(P, salt || INT(1)).
    let mut s = Vec::with_capacity(salt.len() + 4);
    s.extend_from_slice(salt);
    s.extend_from_slice(&1u32.to_be_bytes());
    let mut u = hmac(&s);
    let mut t = u;
    for _ in 1..iterations.max(1) {
        u = hmac(&u);
        for (a, b) in t.iter_mut().zip(u.iter()) {
            *a ^= b;
        }
    }
    t
}

fn hex(bytes: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(D[(b >> 4) as usize] as char);
        s.push(D[(b & 15) as usize] as char);
    }
    s
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let nib = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    s.as_bytes()
        .chunks(2)
        .map(|p| Some(nib(p[0])? << 4 | nib(p[1])?))
        .collect()
}

/// Constant-time equality of two byte strings (the length is not secret).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut d = 0u8;
    for (x, y) in a.iter().zip(b) {
        d |= x ^ y;
    }
    d == 0
}

/// Why a password cannot be set.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PasswordError {
    TooShort,
    TooLong,
    /// Contains a NUL or control character (it would not survive being typed back).
    BadChar,
    /// Salt is empty or longer than 64 bytes.
    BadSalt,
}

/// Shortest password `hash` accepts.
pub const MIN_PASSWORD: usize = 4;

/// Hash `password` with `salt` and `iterations` into the stored form.
pub fn hash_with(password: &str, salt: &[u8], iterations: u32) -> Result<String, PasswordError> {
    if password.len() < MIN_PASSWORD {
        return Err(PasswordError::TooShort);
    }
    if password.len() > MAX_PASSWORD {
        return Err(PasswordError::TooLong);
    }
    if password.chars().any(|c| c.is_control()) {
        return Err(PasswordError::BadChar);
    }
    if salt.is_empty() || salt.len() > 64 {
        return Err(PasswordError::BadSalt);
    }
    let it = iterations.clamp(MIN_ITERATIONS, MAX_ITERATIONS);
    let h = pbkdf2(password.as_bytes(), salt, it);
    Ok(alloc::format!("{TAG}{it}${}${}", hex(salt), hex(&h)))
}

/// Hash `password` with the default cost; `salt` comes from the system entropy.
pub fn hash(password: &str, salt: &[u8; SALT_LEN]) -> Result<String, PasswordError> {
    hash_with(password, salt, DEFAULT_ITERATIONS)
}

struct Parsed {
    iterations: u32,
    salt: Vec<u8>,
    hash: Vec<u8>,
}

fn parse(stored: &str) -> Option<Parsed> {
    let rest = stored.strip_prefix(TAG)?;
    let mut it = rest.split('$');
    let iterations: u32 = it.next()?.parse().ok()?;
    let salt = unhex(it.next()?)?;
    let hash = unhex(it.next()?)?;
    if it.next().is_some() || salt.is_empty() || salt.len() > 64 || hash.len() != OUT {
        return None;
    }
    if !(MIN_ITERATIONS..=MAX_ITERATIONS).contains(&iterations) {
        return None;
    }
    Some(Parsed {
        iterations,
        salt,
        hash,
    })
}

/// Is `stored` a well-formed hash this module can verify?
pub fn is_valid_hash(stored: &str) -> bool {
    parse(stored).is_some()
}

/// Does `password` match the `stored` hash? A malformed hash never matches.
pub fn verify(password: &str, stored: &str) -> bool {
    let Some(p) = parse(stored) else {
        return false;
    };
    if password.len() > MAX_PASSWORD {
        return false;
    }
    let h = pbkdf2(password.as_bytes(), &p.salt, p.iterations);
    ct_eq(&h, &p.hash)
}

/// Should a hash that just verified be replaced with one at the current cost?
pub fn needs_rehash(stored: &str) -> bool {
    parse(stored).is_some_and(|p| p.iterations < DEFAULT_ITERATIONS)
}

#[cfg(test)]
mod tests;
