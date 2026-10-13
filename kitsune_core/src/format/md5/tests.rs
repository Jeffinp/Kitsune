use super::*;

fn h(s: &[u8]) -> std::string::String {
    std::string::String::from_utf8(hex(&digest(s)).to_vec()).unwrap()
}

#[test]
fn rfc1321_vectors() {
    assert_eq!(h(b""), "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(h(b"a"), "0cc175b9c0f1b6a831c399e269772661");
    assert_eq!(h(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(h(b"message digest"), "f96b697d7cb7938d525a2f31aaf161d0");
    assert_eq!(
        h(b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"),
        "57edf4a22be3c955ac49da2e2107b67a"
    );
}

#[test]
fn streaming_matches_one_shot() {
    let data: std::vec::Vec<u8> = (0..1000u32).map(|i| (i * 7 + 3) as u8).collect();
    for split in [0, 1, 55, 56, 63, 64, 65, 127, 999] {
        let mut m = Md5::new();
        m.update(&data[..split]);
        m.update(&data[split..]);
        assert_eq!(m.finish(), digest(&data), "split {split}");
    }
}
