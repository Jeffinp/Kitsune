//! The verdict for one request.

use super::Kind;
use super::lists::{AD_HOSTS, AD_PATHS, TRACKER_HOSTS, TRACKER_PATHS, any_pattern, contains_line};
use super::urlparts::{HOST_MAX, lower_host, site_of, split};

/// What kind of ad or tracker the sub-resource `url` of a page on `page_host` is, or `None` to let
/// it through. `url` is absolute; anything that is not `http://` or `https://` (a `data:` picture,
/// a malformed address) is never blocked. Never allocates.
pub fn classify(page_host: &[u8], url: &[u8]) -> Option<Kind> {
    let (host, rest) = split(url)?;
    let mut hb = [0u8; HOST_MAX];
    let host = lower_host(host, &mut hb)?;
    let mut pb = [0u8; HOST_MAX];
    let third = match lower_host(page_host, &mut pb) {
        Some(p) => site_of(p) != site_of(host),
        None => true,
    };
    // Host rules: the host itself, then each parent down to (not including) the bare TLD.
    if third {
        let mut suffix = host;
        loop {
            if contains_line(AD_HOSTS, suffix) {
                return Some(Kind::Ad);
            }
            if contains_line(TRACKER_HOSTS, suffix) {
                return Some(Kind::Tracker);
            }
            match suffix.iter().position(|&c| c == b'.') {
                Some(dot) if suffix[dot + 1..].contains(&b'.') => suffix = &suffix[dot + 1..],
                _ => break,
            }
        }
    }
    // Path rules apply to any request (a first-party ad is still an ad).
    if any_pattern(AD_PATHS, rest) {
        return Some(Kind::Ad);
    }
    if any_pattern(TRACKER_PATHS, rest) {
        return Some(Kind::Tracker);
    }
    None
}
