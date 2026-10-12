//! Numbers and lengths in SVG attributes: decimals as 16.16 fixed point.

/// A decimal number (sign, digits, fraction, exponent) as 16.16 fixed point, and the rest of `s`.
pub(super) fn num(s: &str) -> Option<(i64, &str)> {
    let b = s.as_bytes();
    let mut i = 0;
    let neg = match b.first() {
        Some(b'-') => {
            i = 1;
            true
        }
        Some(b'+') => {
            i = 1;
            false
        }
        _ => false,
    };
    let start = i;
    let mut int: i64 = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        int = (int * 10 + i64::from(b[i] - b'0')).min(1 << 30);
        i += 1;
    }
    let mut frac: i64 = 0;
    let mut digits = i - start;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let mut scale: i64 = 65536;
        while i < b.len() && b[i].is_ascii_digit() {
            scale /= 10;
            frac += i64::from(b[i] - b'0') * scale;
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return None;
    }
    let mut v = int * 65536 + frac;
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        let eneg = b.get(j) == Some(&b'-');
        if matches!(b.get(j), Some(b'-' | b'+')) {
            j += 1;
        }
        let es = j;
        let mut e = 0i32;
        while j < b.len() && b[j].is_ascii_digit() {
            e = (e * 10 + i32::from(b[j] - b'0')).min(20);
            j += 1;
        }
        if j > es {
            for _ in 0..e {
                v = if eneg {
                    v / 10
                } else {
                    v.saturating_mul(10).min(1 << 46)
                };
            }
            i = j;
        }
    }
    Some((if neg { -v } else { v }, &s[i..]))
}

pub(super) fn skip_sep(s: &str) -> &str {
    s.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == ',')
}

/// A length attribute (`12`, `12px`, `50%`) as 16.16; percentages need `base`.
pub(super) fn length(s: &str, base: i64) -> Option<i64> {
    let (n, rest) = num(s.trim())?;
    Some(match rest.trim() {
        "" | "px" => n,
        "pt" => n * 4 / 3,
        "pc" => n * 16,
        "in" => n * 96,
        "mm" => n * 96 * 10 / 254,
        "cm" => n * 96 * 100 / 254,
        "em" => n * 16,
        "%" => n * base / 100,
        _ => return None,
    })
}
