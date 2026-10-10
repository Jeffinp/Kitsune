use super::*;
use crate::apps::fileman::archive::{ArchiveError, compress, extract_zip, is_zip};
use crate::format::zip::{ZipError, ZipWriter};

fn tree() -> Fs3<RamDisk> {
    let mut fs = fresh();
    fs.mkdir(b"/w", NOW).unwrap();
    fs.write_file(b"/w/a.txt", b"hello hello hello hello", NOW)
        .unwrap();
    fs.mkdir(b"/w/src", NOW).unwrap();
    fs.write_file(b"/w/src/main.rs", b"fn main() {}", NOW)
        .unwrap();
    fs.mkdir(b"/w/src/empty", NOW).unwrap();
    fs
}

#[test]
fn compress_then_extract_rebuilds_the_tree_in_a_new_folder() {
    let mut fs = tree();
    let items = vec![b"/w/src".to_vec(), b"/w/a.txt".to_vec()];
    let zip = compress(&mut fs, &items, "Arquivo", NOW).unwrap();
    assert_eq!(zip, b"/w/Arquivo.zip");
    let one = compress(&mut fs, &[b"/w/src".to_vec()], "x", NOW).unwrap();
    assert_eq!(one, b"/w/src.zip", "one item names the archive");
    let r = extract_zip(&mut fs, &zip, NOW).unwrap();
    assert_eq!(
        (r.folder.as_slice(), r.files, r.skipped),
        (&b"/w/Arquivo"[..], 2, 0)
    );
    assert_eq!(
        fs.read_file(b"/w/Arquivo/src/main.rs").unwrap(),
        b"fn main() {}"
    );
    assert_eq!(
        fs.read_file(b"/w/Arquivo/a.txt").unwrap(),
        b"hello hello hello hello"
    );
    assert!(
        Backend::stat(&mut fs, b"/w/Arquivo/src/empty")
            .unwrap()
            .kind
            == EntryKind::Dir
    );
    // Extracting again does not touch the first folder.
    let r = extract_zip(&mut fs, &zip, NOW).unwrap();
    assert_eq!(r.folder, b"/w/Arquivo (2)");
    // A second compress does not overwrite either.
    let again = compress(&mut fs, &items, "Arquivo", NOW).unwrap();
    assert_eq!(again, b"/w/Arquivo (2).zip");
}

#[test]
fn unsafe_names_are_left_out_and_the_rest_is_extracted() {
    let mut fs = fresh();
    let mut w = ZipWriter::new();
    w.add_file("../escape.txt", b"x", (0, 0x21)).unwrap();
    w.add_file("C:\\win.txt", b"x", (0, 0x21)).unwrap();
    w.add_file("fine/ok.txt", b"ok", (0, 0x21)).unwrap();
    fs.write_file(b"/pack.zip", &w.finish(), NOW).unwrap();
    let r = extract_zip(&mut fs, b"/pack.zip", NOW).unwrap();
    assert_eq!((r.files, r.skipped), (1, 2));
    assert_eq!(fs.read_file(b"/pack/fine/ok.txt").unwrap(), b"ok");
    assert!(fs.stat(b"/escape.txt").is_err() && fs.stat(b"/pack/escape.txt").is_err());
}

#[test]
fn a_file_that_is_not_a_zip_is_reported() {
    let mut fs = fresh();
    fs.write_file(b"/x.zip", b"this is plain text, not an archive", NOW)
        .unwrap();
    assert_eq!(
        extract_zip(&mut fs, b"/x.zip", NOW),
        Err(ArchiveError::Zip(ZipError::NotZip))
    );
    assert_eq!(
        extract_zip(&mut fs, b"/missing.zip", NOW),
        Err(ArchiveError::Fs(VfsError::NotFound))
    );
    // Nothing was created for the failed one.
    assert!(fs.stat(b"/x").is_err());
}

#[test]
fn a_damaged_entry_is_skipped_not_fatal() {
    let mut fs = fresh();
    let mut w = ZipWriter::new();
    w.add_file("good.txt", b"good good good good good", (0, 0x21))
        .unwrap();
    w.add_file("bad.txt", b"bad bad bad bad bad bad bad", (0, 0x21))
        .unwrap();
    let mut zip = w.finish();
    // Flip a byte in the second entry's data (after its 30-byte header and name).
    let at = zip.windows(7).position(|w| w == b"bad.txt").unwrap() + 7;
    zip[at] ^= 0x55;
    fs.write_file(b"/p.zip", &zip, NOW).unwrap();
    let r = extract_zip(&mut fs, b"/p.zip", NOW).unwrap();
    assert_eq!((r.files, r.skipped), (1, 1));
    assert!(fs.read_file(b"/p/good.txt").is_ok());
}

#[test]
fn names_and_menu() {
    assert!(is_zip(b"a.zip") && is_zip(b"A.ZIP") && !is_zip(b".zip") && !is_zip(b"a.zip.txt"));
    let ctx = |zip, selected| MenuCtx {
        zip,
        in_trash: false,
        in_apps: false,
        app_installed: false,
        selected,
        image: false,
        clip_has_items: false,
    };
    let cmds = |c| {
        context_menu(c)
            .into_iter()
            .map(|(c, _)| c)
            .collect::<Vec<_>>()
    };
    assert!(cmds(ctx(true, 1)).contains(&Cmd::ExtractHere));
    assert!(!cmds(ctx(false, 1)).contains(&Cmd::ExtractHere));
    assert!(!cmds(ctx(true, 2)).contains(&Cmd::ExtractHere));
    assert!(cmds(ctx(false, 2)).contains(&Cmd::Compress));
    assert!(!cmds(ctx(false, 0)).contains(&Cmd::Compress));
}
