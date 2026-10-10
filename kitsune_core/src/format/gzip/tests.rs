use super::*;

// Generated with python3's gzip/zlib (mtime 0); see tools in the commit.
const GZ_SMALL: &str = "1f8b08000000000002ffb3c928c9cdb1b349ca4fa9b4b3c930b4f3cf49b4d107d2360576e95599050ae5f945d9c50a99790ade9925c5a579a936fa057636fa10e5fa60bd007f2c505342000000";
const GZ_NAMED: &str = "1f8b081c00000000000306004142020078796e616d652e747874006120636f6d6d656e7400b3c928c9cdb1b349ca4fa9b4b3c930b4f3cf49b4d107d2360576e95599050ae5f945d9c50a99790ade9925c5a579a936fa057636fa10e5fa60bd007f2c505342000000";
const ZLIB_SMALL: &str = "78dab3c928c9cdb1b349ca4fa9b4b3c930b4f3cf49b4d107d2360576e95599050ae5f945d9c50a99790ade9925c5a579a936fa057636fa10e5fa60bd00f0c1168b";
const RAW_SMALL: &str = "b3c928c9cdb1b349ca4fa9b4b3c930b4f3cf49b4d107d2360576e95599050ae5f945d9c50a99790ade9925c5a579a936fa057636fa10e5fa60bd00";
const GZ_BIG: &str = "1f8b08000000000000ffedc9b10900200c00b057fcc0074a7f71e85ec4ff71f60421538644e7aeae75c643ccce504a29a594524a29a594524a29a594524a29a594524a29a59452ffd705e5abee21302a0000";
const BIG_LEN: usize = 10800;
const TEXT: &[u8] = b"<html><body><h1>Ola</h1><p>gzip works in Kitsune</p></body></html>";

fn hex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

#[test]
fn gunzip_small() {
    assert_eq!(gunzip(&hex(GZ_SMALL), 4096).unwrap(), TEXT);
}

#[test]
fn gunzip_with_extra_name_and_comment_fields() {
    assert_eq!(gunzip(&hex(GZ_NAMED), 4096).unwrap(), TEXT);
}

#[test]
fn gunzip_repetitive_body() {
    let out = gunzip(&hex(GZ_BIG), 1 << 20).unwrap();
    assert_eq!(out.len(), BIG_LEN);
    assert!(out.starts_with(b"<p>repeat repeat repeat</p>"));
}

#[test]
fn output_limit_stops_a_compression_bomb() {
    assert_eq!(gunzip(&hex(GZ_BIG), 1000), Err(EncodingError::TooLarge));
    assert_eq!(
        gunzip(&hex(GZ_BIG), BIG_LEN - 1),
        Err(EncodingError::TooLarge)
    );
    assert!(gunzip(&hex(GZ_BIG), BIG_LEN).is_ok());
}

#[test]
fn bad_magic_and_method() {
    let mut v = hex(GZ_SMALL);
    v[0] = 0;
    assert_eq!(gunzip(&v, 4096), Err(EncodingError::BadHeader));
    let mut v = hex(GZ_SMALL);
    v[2] = 7;
    assert_eq!(gunzip(&v, 4096), Err(EncodingError::BadHeader));
}

#[test]
fn reserved_flag_bits_refused() {
    let mut v = hex(GZ_SMALL);
    v[3] = 0x20;
    assert_eq!(gunzip(&v, 4096), Err(EncodingError::BadHeader));
}

#[test]
fn corrupted_crc_or_length_detected() {
    let mut v = hex(GZ_SMALL);
    let n = v.len();
    v[n - 8] ^= 1; // CRC
    assert_eq!(gunzip(&v, 4096), Err(EncodingError::BadChecksum));
    let mut v = hex(GZ_SMALL);
    v[n - 1] ^= 1; // ISIZE
    assert_eq!(gunzip(&v, 4096), Err(EncodingError::BadChecksum));
}

#[test]
fn truncation_at_every_length_is_an_error_not_a_panic() {
    let v = hex(GZ_SMALL);
    for cut in 0..v.len() {
        assert!(gunzip(&v[..cut], 4096).is_err(), "cut {cut}");
    }
    let v = hex(GZ_NAMED);
    for cut in 0..v.len() {
        assert!(gunzip(&v[..cut], 4096).is_err(), "named cut {cut}");
    }
}

#[test]
fn corrupted_deflate_data_is_detected() {
    let mut v = hex(GZ_SMALL);
    v[14] ^= 0xFF;
    assert!(gunzip(&v, 4096).is_err());
}

#[test]
fn trailing_garbage_after_member_is_ignored() {
    let mut v = hex(GZ_SMALL);
    v.extend_from_slice(b"garbage");
    assert_eq!(gunzip(&v, 4096).unwrap(), TEXT);
}

#[test]
fn unterminated_name_field_is_truncated() {
    let mut v = vec![0x1F, 0x8B, 8, FNAME, 0, 0, 0, 0, 0, 3];
    v.extend_from_slice(&[b'a'; 30]); // no NUL
    assert_eq!(gunzip(&v, 4096), Err(EncodingError::Truncated));
}

#[test]
fn huge_extra_length_does_not_overflow() {
    let mut v = vec![0x1F, 0x8B, 8, FEXTRA, 0, 0, 0, 0, 0, 3, 0xFF, 0xFF];
    v.extend_from_slice(&[0; 10]);
    assert_eq!(gunzip(&v, 4096), Err(EncodingError::Truncated));
}

#[test]
fn http_deflate_accepts_zlib_and_raw() {
    assert_eq!(inflate_http_deflate(&hex(ZLIB_SMALL), 4096).unwrap(), TEXT);
    assert_eq!(inflate_http_deflate(&hex(RAW_SMALL), 4096).unwrap(), TEXT);
    assert!(inflate_http_deflate(b"not deflate at all!!", 4096).is_err());
    assert_eq!(
        inflate_http_deflate(&hex(ZLIB_SMALL), 10),
        Err(EncodingError::TooLarge)
    );
}

#[test]
fn encoding_header_values() {
    assert_eq!(Encoding::parse(b"gzip"), Encoding::Gzip);
    assert_eq!(Encoding::parse(b" GZIP "), Encoding::Gzip);
    assert_eq!(Encoding::parse(b"x-gzip"), Encoding::Gzip);
    assert_eq!(Encoding::parse(b"deflate"), Encoding::Deflate);
    assert_eq!(Encoding::parse(b"identity"), Encoding::Identity);
    assert_eq!(Encoding::parse(b""), Encoding::Identity);
    assert_eq!(Encoding::parse(b"br"), Encoding::Unsupported);
    assert_eq!(Encoding::parse(b"gzip, br"), Encoding::Unsupported);
}

#[test]
fn decode_body_dispatch() {
    assert_eq!(decode_body(Encoding::Identity, b"abc").unwrap(), b"abc");
    assert_eq!(decode_body(Encoding::Gzip, &hex(GZ_SMALL)).unwrap(), TEXT);
    assert_eq!(
        decode_body(Encoding::Deflate, &hex(ZLIB_SMALL)).unwrap(),
        TEXT
    );
    assert!(decode_body(Encoding::Unsupported, b"x").is_err());
}

// ---- partial decoding (cut streams, bad trailers, stacked codings) ----

fn sample(len: usize) -> Vec<u8> {
    let mut v = Vec::new();
    let mut i = 0u32;
    while v.len() < len {
        v.extend_from_slice(
            alloc::format!("<li>{} {:08x}</li>\n", i, i.wrapping_mul(2_654_435_761)).as_bytes(),
        );
        i += 1;
    }
    v
}

fn gz_of(plain: &[u8]) -> Vec<u8> {
    let mut v = vec![0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 3];
    v.extend_from_slice(&crate::format::deflate::deflate_fixed(plain));
    v.extend_from_slice(&inflate::crc32(plain).to_le_bytes());
    v.extend_from_slice(&(plain.len() as u32).to_le_bytes());
    v
}

#[test]
fn gunzip_partial_of_every_cut_is_a_prefix() {
    let plain = sample(30_000);
    let gz = gz_of(&plain);
    let mut last = 0;
    for cut in 10..gz.len() {
        match gunzip_partial(&gz[..cut], 1 << 20) {
            Ok(d) => {
                assert_eq!(d.status, Completeness::Cut, "cut {cut}");
                assert!(plain.starts_with(&d.data), "cut {cut}");
                assert!(d.data.len() >= last);
                last = d.data.len();
            }
            // nothing decoded yet (the first block header is incomplete)
            Err(e) => assert_eq!(e, EncodingError::Truncated, "cut {cut}"),
        }
    }
    let whole = gunzip_partial(&gz, 1 << 20).unwrap();
    assert_eq!((whole.status, whole.data), (Completeness::Complete, plain));
    // the strict API keeps rejecting every cut
    for cut in 10..gz.len() {
        assert!(gunzip(&gz[..cut], 1 << 20).is_err());
    }
}

#[test]
fn gunzip_partial_reports_checksum_and_size_limit() {
    let plain = sample(5000);
    let mut gz = gz_of(&plain);
    let n = gz.len();
    gz[n - 5] ^= 0x80; // CRC byte
    let d = gunzip_partial(&gz, 1 << 20).unwrap();
    assert_eq!(
        (d.status, d.data.as_slice()),
        (Completeness::BadChecksum, &plain[..])
    );
    let gz = gz_of(&plain);
    let d = gunzip_partial(&gz, 1000).unwrap();
    assert_eq!(d.status, Completeness::TooLarge);
    assert_eq!(d.data, &plain[..1000]);
    // trailer missing: the data is all there, the status says the end was cut
    let d = gunzip_partial(&gz[..gz.len() - 8], 1 << 20).unwrap();
    assert_eq!((d.status, d.data), (Completeness::Cut, plain));
}

#[test]
fn gunzip_partial_errors_only_without_any_output() {
    assert_eq!(gunzip_partial(b"", 100), Err(EncodingError::Truncated));
    assert_eq!(
        gunzip_partial(&[0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0], 100),
        Err(EncodingError::Truncated)
    );
    assert_eq!(
        gunzip_partial(b"not gzip at all", 100),
        Err(EncodingError::BadHeader)
    );
    // a header followed by a reserved block type: damaged from the first byte
    let mut v = vec![0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 3];
    v.extend_from_slice(&[0x07, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(gunzip_partial(&v, 100), Err(EncodingError::Corrupt));
}

#[test]
fn http_deflate_partial_keeps_zlib_and_raw_prefixes() {
    let plain = sample(20_000);
    let z = crate::format::deflate::zlib_compress(&plain);
    let d = inflate_http_deflate_partial(&z[..z.len() / 2], 1 << 20).unwrap();
    assert_eq!(d.status, Completeness::Cut);
    assert!(d.data.len() > 1000 && plain.starts_with(&d.data));
    let raw = crate::format::deflate::deflate_fixed(&plain);
    let d = inflate_http_deflate_partial(&raw[..raw.len() / 2], 1 << 20).unwrap();
    assert_eq!(d.status, Completeness::Cut);
    assert!(d.data.len() > 1000 && plain.starts_with(&d.data));
    // zlib with a wrong Adler: all data, flagged
    let mut z2 = z.clone();
    let n = z2.len();
    z2[n - 2] ^= 1;
    let d = inflate_http_deflate_partial(&z2, 1 << 20).unwrap();
    assert_eq!(
        (d.status, d.data),
        (Completeness::BadChecksum, plain.clone())
    );
    // whole streams are Complete
    assert_eq!(
        inflate_http_deflate_partial(&z, 1 << 20).unwrap().status,
        Completeness::Complete
    );
    assert_eq!(
        inflate_http_deflate_partial(&raw, 1 << 20).unwrap().status,
        Completeness::Complete
    );
}

#[test]
fn coding_chains() {
    use Encoding::*;
    assert_eq!(Encoding::parse_chain(b"gzip"), vec![Gzip]);
    assert_eq!(
        Encoding::parse_chain(b" GZip , Deflate "),
        vec![Gzip, Deflate]
    );
    assert_eq!(Encoding::parse_chain(b"identity"), vec![]);
    assert_eq!(Encoding::parse_chain(b""), vec![]);
    assert_eq!(
        Encoding::parse_chain(b"gzip,,identity,x-gzip"),
        vec![Gzip, Gzip]
    );
    assert_eq!(Encoding::parse_chain(b"br"), vec![Unsupported]);
    assert_eq!(Encoding::parse_chain(b"gzip, zstd"), vec![Unsupported]);
    assert_eq!(Encoding::parse_chain(b"gzip,gzip,gzip,gzip"), vec![Gzip; 4]);
    assert_eq!(
        Encoding::parse_chain(b"gzip,gzip,gzip,gzip,gzip"),
        vec![Unsupported]
    );
}

#[test]
fn stacked_codings_decode_outermost_last_applied_first() {
    let plain = sample(3000);
    let inner = crate::format::deflate::zlib_compress(&plain); // applied first: deflate
    let outer = gz_of(&inner); // then gzip
    let d = decode_chain_partial(&[Encoding::Deflate, Encoding::Gzip], &outer).unwrap();
    assert_eq!((d.status, d.data), (Completeness::Complete, plain));
    // a cut outer stage taints the status of the whole chain
    let d = decode_chain_partial(
        &[Encoding::Deflate, Encoding::Gzip],
        &outer[..outer.len() / 2],
    )
    .unwrap();
    assert_ne!(d.status, Completeness::Complete);
}

#[test]
fn gzip_round_trips_and_python_reads_the_header() {
    for data in [
        &b""[..],
        TEXT,
        &[0u8; 5000][..],
        &(0..=255u8).collect::<Vec<_>>()[..],
    ] {
        let z = gzip(data);
        assert_eq!(&z[..4], &[0x1F, 0x8B, 8, 0]);
        assert_eq!(gunzip(&z, 1 << 20).unwrap(), data);
    }
    assert!(gzip(&[0u8; 5000]).len() < 100, "repetitive data shrinks");
}
