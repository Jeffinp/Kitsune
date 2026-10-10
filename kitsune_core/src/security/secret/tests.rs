use super::*;

#[test]
fn it_edits_like_a_small_string() {
    let mut s = Secret::new();
    assert!(s.is_empty());
    for c in "senha1ç".chars() {
        assert!(s.push(c));
    }
    assert_eq!((s.as_str(), s.chars(), s.len()), ("senha1ç", 7, 8));
    assert!(s.pop());
    assert_eq!(s.as_str(), "senha1");
    s.clear();
    assert!(s.is_empty() && !s.pop());
}

#[test]
fn it_never_grows_past_its_capacity_and_never_reallocates() {
    let mut s = Secret::with_capacity(8);
    let before = s.buf.as_ptr();
    for _ in 0..20 {
        let _ = s.push('x');
    }
    assert_eq!(s.len(), 8);
    assert!(!s.push('y'));
    assert!(!s.push('é') || s.len() <= 8);
    assert_eq!(
        s.buf.as_ptr(),
        before,
        "same allocation: no stray copy left behind"
    );
    // A multi-byte character that does not fit is refused whole.
    let mut t = Secret::with_capacity(3);
    assert!(t.push('a') && t.push('b'));
    assert!(!t.push('é'));
    assert_eq!(t.as_str(), "ab");
}

#[test]
fn debug_does_not_print_the_content() {
    let mut s = Secret::new();
    s.push('q');
    s.push('z');
    let text = alloc::format!("{s:?}");
    assert_eq!(text, "Secret(2 chars)");
    assert!(!text.contains('q') && !text.contains('z'));
}

#[test]
fn duplicate_is_independent() {
    let mut a = Secret::new();
    a.push('z');
    let b = a.duplicate();
    a.clear();
    assert_eq!(b.as_str(), "z");
}
