use super::*;
use alloc::string::ToString;
use alloc::vec;

const SAMPLE: &[u8] = include_bytes!("testdata/sample.zip");

#[test]
fn a_zip_made_by_another_program_lists_and_extracts() {
    let es = read_dir(SAMPLE).unwrap();
    let names: Vec<_> = es.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        ["leia.txt", "pasta/", "pasta/raw.bin", "acentuação.txt"]
    );
    assert_eq!(es[0].method, 8);
    assert_eq!(es[2].method, 0);
    assert!(es[1].is_dir && !es[0].is_dir);
    assert_eq!(
        extract(SAMPLE, &es[0], 1 << 20).unwrap(),
        "ola mundo\n".repeat(40).as_bytes()
    );
    assert_eq!(
        extract(SAMPLE, &es[2], 1 << 20).unwrap(),
        (0..=255u8).collect::<Vec<_>>()
    );
    assert_eq!(extract(SAMPLE, &es[3], 1 << 20).unwrap(), "é".as_bytes());
    assert_eq!(extract(SAMPLE, &es[1], 1 << 20).unwrap(), Vec::<u8>::new());
    let t = from_dos(es[0].dos.0, es[0].dos.1).unwrap();
    assert_eq!(
        (t.year, t.month, t.day, t.hour, t.minute, t.second),
        (2024, 3, 5, 10, 20, 30)
    );
}

#[test]
fn what_we_write_we_read_back() {
    let mut w = ZipWriter::new();
    let dos = dos_time(&DateTime {
        year: 2025,
        month: 12,
        day: 31,
        hour: 23,
        minute: 59,
        second: 58,
    });
    let text = "linha de texto repetida\n".repeat(500);
    w.add_dir("docs", dos).unwrap();
    w.add_file("docs/a.txt", text.as_bytes(), dos).unwrap();
    w.add_file("vazio", b"", dos).unwrap();
    w.add_file("ação.bin", &[7u8; 3], dos).unwrap();
    let zip = w.finish();
    assert!(
        zip.len() < text.len() / 4,
        "text is compressed: {}",
        zip.len()
    );
    let es = read_dir(&zip).unwrap();
    assert_eq!(es.len(), 4);
    assert_eq!(es[0].name, "docs/");
    assert_eq!(extract(&zip, &es[1], 1 << 20).unwrap(), text.as_bytes());
    assert_eq!(extract(&zip, &es[2], 1 << 20).unwrap(), b"");
    assert_eq!(es[3].name, "ação.bin");
    assert_eq!(extract(&zip, &es[3], 1 << 20).unwrap(), [7, 7, 7]);
    assert_eq!(from_dos(es[1].dos.0, es[1].dos.1).unwrap().second, 58);
    // Incompressible data stays stored.
    let mut w = ZipWriter::new();
    let noise: Vec<u8> = (0..300u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    w.add_file("n", &noise, dos).unwrap();
    let zip = w.finish();
    let es = read_dir(&zip).unwrap();
    assert_eq!(extract(&zip, &es[0], 1 << 20).unwrap(), noise);
}

#[test]
fn an_empty_archive_is_valid() {
    let zip = ZipWriter::new().finish();
    assert_eq!(read_dir(&zip).unwrap(), vec![]);
}

#[test]
fn not_a_zip_and_cuts_never_panic() {
    assert_eq!(read_dir(b""), Err(ZipError::NotZip));
    assert_eq!(
        read_dir(b"hello, this is not a zip file at all"),
        Err(ZipError::NotZip)
    );
    // Every prefix and every single-byte corruption is an error or a listing, never a panic.
    for n in 0..SAMPLE.len() {
        let cut = &SAMPLE[..n];
        if let Ok(es) = read_dir(cut) {
            for e in &es {
                let _ = extract(cut, e, 1 << 16);
            }
        }
    }
    for i in 0..SAMPLE.len() {
        let mut d = SAMPLE.to_vec();
        d[i] ^= 0xFF;
        if let Ok(es) = read_dir(&d) {
            for e in &es {
                let _ = extract(&d, e, 1 << 16);
            }
        }
    }
}

#[test]
fn a_flipped_data_byte_fails_the_checksum() {
    let mut w = ZipWriter::new();
    w.add_file(
        "f",
        b"stored? no: abcabcabcabcabcabcabcabcabcabc",
        (0, 0x21),
    )
    .unwrap();
    let mut zip = w.finish();
    let es = read_dir(&zip).unwrap();
    let at = LOC_LEN + 1; // the first data byte
    zip[at] ^= 0x01;
    assert!(extract(&zip, &es[0], 1 << 20).is_err());
}

#[test]
fn the_declared_size_is_a_hard_ceiling() {
    // A directory that claims 10 bytes for data that inflates to far more must not expand it.
    let mut w = ZipWriter::new();
    w.add_file("bomb", &vec![0u8; 100_000], (0, 0x21)).unwrap();
    let mut zip = w.finish();
    let mut es = read_dir(&zip).unwrap();
    assert_eq!(extract(&zip, &es[0], 1 << 20).unwrap().len(), 100_000);
    assert_eq!(extract(&zip, &es[0], 1000), Err(ZipError::TooLarge));
    es[0].size = 10;
    assert_eq!(extract(&zip, &es[0], 1 << 20), Err(ZipError::BadChecksum));
    // The limit is checked before anything is inflated.
    zip.truncate(zip.len());
}

#[test]
fn encryption_zip64_and_odd_methods_are_refused() {
    let mut w = ZipWriter::new();
    w.add_file("f", b"abcabcabcabcabcabc", (0, 0x21)).unwrap();
    let zip = w.finish();
    let mut es = read_dir(&zip).unwrap();
    es[0].method = 12; // bzip2
    assert_eq!(extract(&zip, &es[0], 1 << 20), Err(ZipError::Unsupported));
    es[0].method = 8;
    es[0].flags |= FLAG_ENCRYPTED;
    assert_eq!(extract(&zip, &es[0], 1 << 20), Err(ZipError::Unsupported));
    let mut d = zip.clone();
    let eocd = d.len() - EOCD_LEN;
    d[eocd + 10] = 0xFF;
    d[eocd + 11] = 0xFF;
    d[eocd + 8] = 0xFF;
    d[eocd + 9] = 0xFF;
    assert_eq!(read_dir(&d), Err(ZipError::Unsupported));
}

#[test]
fn names_are_made_safe() {
    let s = |n: &str| safe_path(n);
    assert_eq!(s("a/b.txt").as_deref(), Some("a/b.txt"));
    assert_eq!(s("./a//b/").as_deref(), Some("a/b/"));
    assert_eq!(s("/etc/passwd").as_deref(), Some("etc/passwd"));
    assert_eq!(s("dir\\file.txt").as_deref(), Some("dir/file.txt"));
    assert_eq!(s("../x"), None);
    assert_eq!(s("a/../../x"), None);
    assert_eq!(s("..\\x"), None);
    assert_eq!(s("C:\\Windows"), None);
    assert_eq!(s("a\u{0}b"), None);
    assert_eq!(s("/"), None);
    assert_eq!(s(""), None);
}

#[test]
fn dos_times_round_trip() {
    let t = DateTime {
        year: 1999,
        month: 2,
        day: 28,
        hour: 13,
        minute: 5,
        second: 40,
    };
    let (a, b) = dos_time(&t);
    assert_eq!(from_dos(a, b), Some(t));
    assert_eq!(from_dos(0, 0), None, "month 0 is no date");
    assert!(
        dos_time(&DateTime { year: 1970, ..t }).1 >> 9 == 0,
        "clamped to 1980"
    );
    let _ = t.year.to_string();
}
