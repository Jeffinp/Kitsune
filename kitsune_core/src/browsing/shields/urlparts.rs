//! Taking an absolute address apart without allocating: scheme check, host, path, and the "site" of
//! a host (its registrable domain, by a small heuristic) for the first-party / third-party test.

/// Longest host kept (a DNS name is at most 253 bytes).
pub(super) const HOST_MAX: usize = 255;

fn starts_with_ci(s: &[u8], prefix: &[u8]) -> bool {
    s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// The parts of an `http://` or `https://` address: the authority's host (as written: case and
/// trailing dot untouched, no user-info, no port) and everything from the path on (`/...?...`; may
/// be empty). `None` for any other scheme or an empty host.
pub(super) fn split(url: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut s = url;
    while let [b' ' | b'\t', rest @ ..] = s {
        s = rest;
    }
    let rest = if starts_with_ci(s, b"https://") {
        &s[8..]
    } else if starts_with_ci(s, b"http://") {
        &s[7..]
    } else {
        return None;
    };
    let end = rest
        .iter()
        .position(|&c| matches!(c, b'/' | b'?' | b'#'))
        .unwrap_or(rest.len());
    let (mut auth, tail) = rest.split_at(end);
    if let Some(at) = auth.iter().rposition(|&c| c == b'@') {
        auth = &auth[at + 1..];
    }
    let host = if auth.first() == Some(&b'[') {
        // An IPv6 literal keeps its brackets and colons.
        match auth.iter().position(|&c| c == b']') {
            Some(close) => &auth[..=close],
            None => auth,
        }
    } else {
        match auth.iter().position(|&c| c == b':') {
            Some(colon) => &auth[..colon],
            None => auth,
        }
    };
    if host.is_empty() {
        return None;
    }
    // The fragment is not part of what the server sees.
    let path_end = tail.iter().position(|&c| c == b'#').unwrap_or(tail.len());
    Some((host, &tail[..path_end]))
}

/// Lowercase `host` into `buf` without a trailing dot. `None` when it does not fit.
pub(super) fn lower_host<'a>(host: &[u8], buf: &'a mut [u8; HOST_MAX]) -> Option<&'a [u8]> {
    let host = host.strip_suffix(b".").unwrap_or(host);
    if host.is_empty() || host.len() > HOST_MAX {
        return None;
    }
    for (d, s) in buf.iter_mut().zip(host) {
        *d = s.to_ascii_lowercase();
    }
    Some(&buf[..host.len()])
}

/// Is `host` an IP literal (v4 digits and dots, or anything with a colon or bracket)?
fn is_ip(host: &[u8]) -> bool {
    host.first() == Some(&b'[')
        || host.contains(&b':')
        || (!host.is_empty() && host.iter().all(|&c| c.is_ascii_digit() || c == b'.'))
}

/// Second-level labels that, under a two-letter country code, are part of the public suffix
/// (`co.uk`, `com.br`, `org.au`...): the site is then three labels long.
const SECOND_LEVEL: [&[u8]; 12] = [
    b"co", b"com", b"org", b"net", b"gov", b"edu", b"ac", b"or", b"ne", b"go", b"gob", b"mil",
];

/// The site of a lowercase `host`: its registrable domain, which is the last two labels, or the
/// last three under a country second level (`example.co.uk`). An IP literal is its own site. This
/// is a heuristic (there is no public-suffix list in the image): it errs towards treating two hosts
/// of one organisation as different sites only for unusual suffixes.
pub fn site_of(host: &[u8]) -> &[u8] {
    if is_ip(host) {
        return host;
    }
    let mut dots = host
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, c)| **c == b'.')
        .map(|(i, _)| i);
    let Some(last) = dots.next() else { return host };
    let tld = &host[last + 1..];
    let Some(second) = dots.next() else {
        return host;
    };
    let sld = &host[second + 1..last];
    if tld.len() == 2 && SECOND_LEVEL.contains(&sld) {
        return match dots.next() {
            Some(third) => &host[third + 1..],
            None => host,
        };
    }
    &host[second + 1..]
}

/// Do two hosts (any case) belong to different sites?
pub fn is_third_party(a: &[u8], b: &[u8]) -> bool {
    let (mut ba, mut bb) = ([0u8; HOST_MAX], [0u8; HOST_MAX]);
    match (lower_host(a, &mut ba), lower_host(b, &mut bb)) {
        (Some(x), Some(y)) => site_of(x) != site_of(y),
        // Something unparsable is never "the same site".
        _ => true,
    }
}

/// Does `hay` contain `needle` (which is lowercase), ignoring the ASCII case of `hay`?
pub(super) fn contains_ci(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    hay.windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
}
