//! Time ranges (`TimeRanges` in the DOM): sorted, disjoint intervals of nanoseconds.

use alloc::vec::Vec;

/// A set of time intervals `[start, end)` in nanoseconds, kept sorted and disjoint: two ranges that
/// touch or overlap are one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ranges {
    r: Vec<(i64, i64)>,
}

impl Ranges {
    pub fn new() -> Ranges {
        Ranges::default()
    }

    /// The ranges as given (sorted and merged), `start < end` each.
    pub fn from_pairs(pairs: &[(i64, i64)]) -> Ranges {
        let mut r = Ranges::new();
        for &(a, b) in pairs {
            r.add(a, b);
        }
        r
    }

    pub fn len(&self) -> usize {
        self.r.len()
    }

    pub fn is_empty(&self) -> bool {
        self.r.is_empty()
    }

    pub fn clear(&mut self) {
        self.r.clear();
    }

    /// Start of range `i`.
    pub fn start(&self, i: usize) -> Option<i64> {
        self.r.get(i).map(|x| x.0)
    }

    /// End of range `i`.
    pub fn end(&self, i: usize) -> Option<i64> {
        self.r.get(i).map(|x| x.1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (i64, i64)> + '_ {
        self.r.iter().copied()
    }

    /// The end of the last range.
    pub fn last_end(&self) -> Option<i64> {
        self.r.last().map(|x| x.1)
    }

    /// Add `[a, b)`; empty or reversed intervals are ignored.
    pub fn add(&mut self, a: i64, b: i64) {
        if b <= a {
            return;
        }
        // First range that ends at or after `a` (it may merge), and the first that starts after `b`.
        let i = self.r.partition_point(|x| x.1 < a);
        let j = self.r.partition_point(|x| x.0 <= b);
        if i == j {
            self.r.insert(i, (a, b));
            return;
        }
        let start = a.min(self.r[i].0);
        let end = b.max(self.r[j - 1].1);
        self.r.drain(i + 1..j);
        self.r[i] = (start, end);
    }

    /// Take `[a, b)` out.
    pub fn remove(&mut self, a: i64, b: i64) {
        if b <= a {
            return;
        }
        let mut out = Vec::with_capacity(self.r.len() + 1);
        for &(s, e) in &self.r {
            if e <= a || s >= b {
                out.push((s, e));
                continue;
            }
            if s < a {
                out.push((s, a));
            }
            if e > b {
                out.push((b, e));
            }
        }
        self.r = out;
    }

    /// Is `t` inside a range?
    pub fn contains(&self, t: i64) -> bool {
        self.range_of(t).is_some()
    }

    /// The range holding `t`.
    pub fn range_of(&self, t: i64) -> Option<(i64, i64)> {
        let i = self.r.partition_point(|x| x.1 <= t);
        self.r.get(i).copied().filter(|x| x.0 <= t)
    }

    /// The times in both.
    pub fn intersect(&self, other: &Ranges) -> Ranges {
        let mut out = Vec::new();
        let (mut i, mut j) = (0, 0);
        while i < self.r.len() && j < other.r.len() {
            let (a0, a1) = self.r[i];
            let (b0, b1) = other.r[j];
            let (s, e) = (a0.max(b0), a1.min(b1));
            if s < e {
                out.push((s, e));
            }
            if a1 < b1 {
                i += 1;
            } else {
                j += 1;
            }
        }
        Ranges { r: out }
    }

    /// The same ranges with every gap of at most `slack` closed (a missing frame's worth of time does
    /// not split what a player can play through).
    pub fn closed(&self, slack: i64) -> Ranges {
        let mut out: Vec<(i64, i64)> = Vec::with_capacity(self.r.len());
        for &(s, e) in &self.r {
            match out.last_mut() {
                Some(last) if s - last.1 <= slack => last.1 = last.1.max(e),
                _ => out.push((s, e)),
            }
        }
        Ranges { r: out }
    }

    /// How much is buffered from `t` on without a gap (0 when `t` is not buffered).
    pub fn ahead(&self, t: i64) -> i64 {
        self.range_of(t).map_or(0, |(_, e)| e - t)
    }
}
