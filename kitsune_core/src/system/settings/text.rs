//! Helpers of the `key=value` text form: paths, trimming and integer parsing and printing.

use super::*;

/// The volume path a stored wallpaper path means: the settings file and the
/// settings page accept a bare name (`papel.png`, what the old flat file system
/// used), the volume wants an absolute one (`/papel.png`). An absolute path is
/// returned unchanged.
pub fn absolute_path(p: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(p.len() + 1);
    if p.first() != Some(&b'/') {
        out.push(b'/');
    }
    out.extend_from_slice(p);
    out
}

pub(super) fn valid_path(p: &[u8]) -> bool {
    !p.is_empty()
        && p.len() <= PATH_CAP
        && p.iter()
            .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'))
}

pub(super) fn trim(mut s: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = s {
        if first.is_ascii_whitespace() {
            s = rest;
        } else {
            break;
        }
    }
    while let [rest @ .., last] = s {
        if last.is_ascii_whitespace() {
            s = rest;
        } else {
            break;
        }
    }
    s
}

pub(super) fn parse_u32(s: &[u8]) -> Option<u32> {
    if s.is_empty() || s.len() > 9 || !s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(s.iter().fold(0u32, |a, &d| a * 10 + (d - b'0') as u32))
}

pub(super) fn parse_i32(s: &[u8]) -> Option<i32> {
    match s {
        [b'-', rest @ ..] => parse_u32(rest).map(|v| -(v as i32)),
        [b'+', rest @ ..] => parse_u32(rest).map(|v| v as i32),
        _ => parse_u32(s).map(|v| v as i32),
    }
}

pub(super) fn push_i32(out: &mut Vec<u8>, v: i32) {
    if v < 0 {
        out.push(b'-');
    }
    let mut n = v.unsigned_abs();
    let mut tmp = [0u8; 10];
    let mut i = tmp.len();
    loop {
        i -= 1;
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(&tmp[i..]);
}

pub(super) fn push_kv(out: &mut Vec<u8>, key: &[u8], v: u32) {
    out.extend_from_slice(key);
    out.push(b'=');
    push_i32(out, v as i32);
    out.push(b'\n');
}
