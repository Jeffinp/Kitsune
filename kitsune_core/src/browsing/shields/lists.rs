//! The rule lists, embedded from `shields/data/`, and the searches over them.
//!
//! Host lists are **sorted** (byte order), one lowercase name per line, so a lookup is a binary
//! search over the text itself: no index to build at boot, no allocation. The tests keep the files
//! sorted, unique, lowercase and free of blank lines. Path lists are short and unsorted.

use super::urlparts::contains_ci;

/// Ad networks and ad servers (hosts and their subdomains).
pub(super) const AD_HOSTS: &str = include_str!("data/ad_hosts.txt");
/// Analytics, tag managers and tracking endpoints.
pub(super) const TRACKER_HOSTS: &str = include_str!("data/tracker_hosts.txt");
/// Path fragments of ad requests.
pub(super) const AD_PATHS: &str = include_str!("data/ad_paths.txt");
/// Path fragments of tracking pixels and analytics scripts.
pub(super) const TRACKER_PATHS: &str = include_str!("data/tracker_paths.txt");

/// Is `needle` one of the lines of the sorted text `list`?
pub(super) fn contains_line(list: &str, needle: &[u8]) -> bool {
    let b = list.as_bytes();
    // The window [lo, hi) always starts and ends on a line boundary.
    let (mut lo, mut hi) = (0usize, b.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let start = b[lo..mid]
            .iter()
            .rposition(|&c| c == b'\n')
            .map_or(lo, |p| lo + p + 1);
        let end = b[mid..hi]
            .iter()
            .position(|&c| c == b'\n')
            .map_or(hi, |p| mid + p);
        match b[start..end].cmp(needle) {
            core::cmp::Ordering::Equal => return true,
            core::cmp::Ordering::Less => lo = (end + 1).min(hi),
            core::cmp::Ordering::Greater => hi = start,
        }
    }
    false
}

/// Does `target` (path and query) contain any line of `patterns`?
pub(super) fn any_pattern(patterns: &str, target: &[u8]) -> bool {
    patterns
        .lines()
        .filter(|p| !p.is_empty())
        .any(|p| contains_ci(target, p.as_bytes()))
}
