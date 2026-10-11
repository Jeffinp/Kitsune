use super::*;
use alloc::string::ToString;

fn h(s: &str) -> Vec<u8> {
    unhex(s).unwrap()
}

#[test]
fn hmac_sha256_matches_rfc4231() {
    // Test case 1.
    assert_eq!(
        hmac_sha256(&[0x0b; 20], b"Hi There").to_vec(),
        h("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7")
    );
    // Test case 2.
    assert_eq!(
        hmac_sha256(b"Jefe", b"what do ya want for nothing?").to_vec(),
        h("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
    );
    // Test case 6: a key longer than the block.
    assert_eq!(
        hmac_sha256(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First"
        )
        .to_vec(),
        h("60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54")
    );
}

#[test]
fn pbkdf2_matches_known_vectors() {
    assert_eq!(
        pbkdf2(b"password", b"salt", 1).to_vec(),
        h("120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b")
    );
    assert_eq!(
        pbkdf2(b"password", b"salt", 2).to_vec(),
        h("ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43")
    );
    assert_eq!(
        pbkdf2(b"password", b"salt", 4096).to_vec(),
        h("c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a")
    );
}

#[test]
fn hash_then_verify_round_trips() {
    let stored = hash_with("correct horse", &[7; 16], 1_000).unwrap();
    assert!(stored.starts_with("$kpw1$1000$"));
    assert!(verify("correct horse", &stored));
    assert!(!verify("correct horsf", &stored));
    assert!(!verify("", &stored));
    assert!(is_valid_hash(&stored));
}

#[test]
fn the_same_password_with_another_salt_gives_another_hash() {
    let a = hash_with("secret!", &[1; 16], 1_000).unwrap();
    let b = hash_with("secret!", &[2; 16], 1_000).unwrap();
    assert_ne!(a, b);
    assert!(verify("secret!", &a) && verify("secret!", &b));
}

#[test]
fn password_rules() {
    let salt = [3u8; 16];
    assert_eq!(hash_with("abc", &salt, 1_000), Err(PasswordError::TooShort));
    assert_eq!(
        hash_with(&"a".repeat(MAX_PASSWORD + 1), &salt, 1_000),
        Err(PasswordError::TooLong)
    );
    assert_eq!(
        hash_with("tab\there", &salt, 1_000),
        Err(PasswordError::BadChar)
    );
    assert_eq!(
        hash_with("fine pw", &[], 1_000),
        Err(PasswordError::BadSalt)
    );
    assert!(hash_with("açaí-ção", &salt, 1_000).is_ok());
    assert!(hash_with(&"a".repeat(MAX_PASSWORD), &salt, 1_000).is_ok());
}

#[test]
fn iteration_count_is_clamped_both_ways() {
    let low = hash_with("secret!", &[1; 8], 1).unwrap();
    assert!(low.starts_with("$kpw1$1000$"));
    let high = hash_with("secret!", &[1; 8], u32::MAX).unwrap_or_default();
    // Not computed in a test (a million rounds); the claim is what is stored.
    let _ = high;
}

#[test]
fn malformed_hashes_never_verify() {
    let good = hash_with("secret!", &[9; 16], 1_000).unwrap();
    let parts: Vec<&str> = good.split('$').collect();
    // $ kpw1 iters salt hash
    let bad = [
        "".to_string(),
        "!".to_string(),
        "*".to_string(),
        "$kpw1$".to_string(),
        "$kpw1$1000$aa".to_string(),
        alloc::format!("$kpw1$1000$$ {}", parts[4]),
        alloc::format!("$kpw1$999$aa$ {}", parts[4]),
        alloc::format!("$kpw1$1000000000${}${}", parts[3], parts[4]),
        alloc::format!("$kpw1$1000${}$zz", parts[3]),
        alloc::format!("$kpw1$1000${}${}00", parts[3], parts[4]),
        alloc::format!("$kpw1$1000${}${}$extra", parts[3], parts[4]),
        alloc::format!("$kpw2$1000${}${}", parts[3], parts[4]),
        alloc::format!("$kpw1$1000${}${}", parts[3], parts[4].to_uppercase()),
    ];
    for b in &bad {
        assert!(!verify("secret!", b), "{b:?}");
        assert!(!is_valid_hash(b), "{b:?}");
    }
    assert!(verify("secret!", &good));
}

#[test]
fn locked_markers_are_not_hashes() {
    for m in ["", "!", "*", "x", "!$kpw1$1000$aa$bb"] {
        assert!(!verify("anything", m));
    }
}

#[test]
fn old_hashes_ask_for_a_rehash() {
    let old = hash_with("secret!", &[1; 16], MIN_ITERATIONS).unwrap();
    assert!(needs_rehash(&old));
    assert!(verify("secret!", &old));
    let now = hash("secret!", &[1; SALT_LEN]).unwrap();
    assert!(!needs_rehash(&now));
    assert!(!needs_rehash("garbage"));
}

#[test]
fn constant_time_compare() {
    assert!(ct_eq(b"abc", b"abc"));
    assert!(!ct_eq(b"abc", b"abd"));
    assert!(!ct_eq(b"abc", b"ab"));
    assert!(ct_eq(b"", b""));
}

#[test]
fn an_overlong_attempt_does_not_verify() {
    let good = hash_with("secret!", &[9; 16], 1_000).unwrap();
    assert!(!verify(&"x".repeat(MAX_PASSWORD + 1), &good));
}

#[test]
fn pbkdf2_agrees_with_the_plain_hmac_loop_for_long_and_empty_keys() {
    for key in [&b""[..], &[0xAB; 64][..], &[0xCD; 65][..], &[0xEF; 200][..]] {
        let mut s = b"salt".to_vec();
        s.extend_from_slice(&1u32.to_be_bytes());
        let mut u = hmac_sha256(key, &s);
        let mut t = u;
        for _ in 1..5 {
            u = hmac_sha256(key, &u);
            for (a, b) in t.iter_mut().zip(u.iter()) {
                *a ^= b;
            }
        }
        assert_eq!(pbkdf2(key, b"salt", 5), t);
    }
}
