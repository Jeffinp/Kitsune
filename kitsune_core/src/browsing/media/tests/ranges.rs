use crate::browsing::media::ranges::Ranges;
use alloc::vec::Vec;

fn v(r: &Ranges) -> Vec<(i64, i64)> {
    r.iter().collect()
}

#[test]
fn add_merges_overlapping_and_touching_ranges() {
    let mut r = Ranges::new();
    r.add(10, 20);
    r.add(30, 40);
    assert_eq!(v(&r), [(10, 20), (30, 40)]);
    r.add(20, 25); // touches the first
    assert_eq!(v(&r), [(10, 25), (30, 40)]);
    r.add(24, 31); // bridges both
    assert_eq!(v(&r), [(10, 40)]);
    r.add(0, 5);
    r.add(50, 60);
    assert_eq!(v(&r), [(0, 5), (10, 40), (50, 60)]);
    r.add(-10, 100);
    assert_eq!(v(&r), [(-10, 100)]);
    // Empty and reversed intervals are nothing.
    r.add(200, 200);
    r.add(300, 250);
    assert_eq!(r.len(), 1);
}

#[test]
fn remove_cuts_and_splits() {
    let mut r = Ranges::from_pairs(&[(0, 100), (200, 300)]);
    r.remove(50, 250);
    assert_eq!(v(&r), [(0, 50), (250, 300)]);
    r.remove(10, 20);
    assert_eq!(v(&r), [(0, 10), (20, 50), (250, 300)]);
    r.remove(-5, 1000);
    assert!(r.is_empty());
}

#[test]
fn contains_ahead_and_range_of() {
    let r = Ranges::from_pairs(&[(0, 100), (200, 300)]);
    assert!(r.contains(0));
    assert!(r.contains(99));
    assert!(!r.contains(100), "the end is not inside");
    assert!(!r.contains(150));
    assert_eq!(r.range_of(250), Some((200, 300)));
    assert_eq!(r.ahead(250), 50);
    assert_eq!(r.ahead(150), 0);
    assert_eq!(r.last_end(), Some(300));
    assert_eq!(
        (r.start(1), r.end(1), r.start(2)),
        (Some(200), Some(300), None)
    );
}

#[test]
fn intersect_and_close() {
    let a = Ranges::from_pairs(&[(0, 100), (200, 300), (400, 500)]);
    let b = Ranges::from_pairs(&[(50, 250), (290, 450)]);
    assert_eq!(
        v(&a.intersect(&b)),
        [(50, 100), (200, 250), (290, 300), (400, 450)]
    );
    assert!(a.intersect(&Ranges::new()).is_empty());
    let c = Ranges::from_pairs(&[(0, 10), (15, 20), (100, 110)]);
    assert_eq!(v(&c.closed(5)), [(0, 20), (100, 110)]);
    assert_eq!(v(&c.closed(4)), [(0, 10), (15, 20), (100, 110)]);
}
