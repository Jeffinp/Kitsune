//! `gzip`, `gunzip`, `zip` and `unzip`.

use super::*;

fn project() -> T {
    let mut t = T::new();
    t.fs = MemFs::new()
        .with_dir("/w")
        .with_file("/w/a.txt", "hello hello hello hello\n".as_bytes())
        .with_dir("/w/src")
        .with_file("/w/src/main.rs", b"fn main() {}\n")
        .with_dir("/w/src/deep")
        .with_file("/w/src/deep/x", b"x");
    t.run("cd /w");
    t
}

#[test]
fn gzip_and_gunzip_round_trip_and_remove_the_original() {
    let mut t = project();
    assert_eq!(t.run("gzip a.txt").status, 0);
    assert!(t.fs.stat("/w/a.txt").is_err());
    assert!(t.fs.stat("/w/a.txt.gz").is_ok());
    // The result is a real gzip stream: cat | gunzip -c reads it back.
    assert_eq!(t.out("gunzip -c a.txt.gz"), "hello hello hello hello\n");
    assert_eq!(t.run("gunzip a.txt.gz").status, 0);
    assert_eq!(t.out("cat a.txt"), "hello hello hello hello\n");
    assert!(t.fs.stat("/w/a.txt.gz").is_err());
}

#[test]
fn gzip_keeps_with_k_and_refuses_to_overwrite() {
    let mut t = project();
    assert_eq!(t.run("gzip -k a.txt").status, 0);
    assert!(t.fs.stat("/w/a.txt").is_ok());
    let r = t.run("gzip -k a.txt");
    assert_eq!(r.status, 1);
    assert!(r.text().contains("already exists"), "{}", r.text());
    assert_eq!(t.run("gzip -kf a.txt").status, 0);
    // Not .gz, not gzip, a directory.
    let r = t.run("gunzip a.txt");
    assert!(r.text().contains("unknown suffix"), "{}", r.text());
    t.fs.write("/w/bad.gz", b"not gzip").unwrap();
    let r = t.run("gunzip bad.gz");
    assert!(r.text().contains("not in gzip format"), "{}", r.text());
    assert!(t.run("gzip src").text().contains("is a directory"));
    assert!(t.run("gzip a.txt.gz").text().contains("already has"));
}

#[test]
fn gzip_works_on_a_pipe() {
    let mut t = project();
    assert_eq!(t.out("echo piped | gzip | gunzip"), "piped\n");
}

#[test]
fn zip_then_unzip_rebuilds_the_tree() {
    let mut t = project();
    let r = t.run("zip -r out src a.txt");
    assert_eq!(r.status, 0, "{}", r.text());
    assert!(r.text().contains("adding: src/deep/x"), "{}", r.text());
    assert!(t.fs.stat("/w/out.zip").is_ok(), "the suffix is added");
    let l = t.out("unzip -l out.zip");
    for name in [
        "src/",
        "src/deep/",
        "src/deep/x",
        "src/main.rs",
        "a.txt",
        "5 items",
    ] {
        assert!(l.contains(name), "{name} in {l}");
    }
    assert_eq!(t.run("unzip out.zip -d /copy").status, 0);
    assert_eq!(t.out("cat /copy/src/main.rs"), "fn main() {}\n");
    assert_eq!(t.out("cat /copy/src/deep/x"), "x");
    assert_eq!(t.out("cat /copy/a.txt"), "hello hello hello hello\n");
}

#[test]
fn zip_without_r_adds_only_the_folder() {
    let mut t = project();
    assert_eq!(t.run("zip z src").status, 0);
    let l = t.out("unzip -l z.zip");
    assert!(l.contains("src/") && !l.contains("main.rs"), "{l}");
    // The archive does not contain itself.
    assert_eq!(t.run("zip -r all .").status, 0);
}

#[test]
fn unzip_picks_members_and_does_not_overwrite_without_o() {
    let mut t = project();
    t.run("zip -r z src a.txt");
    assert_eq!(t.run("unzip z.zip a.txt -d /one").status, 0);
    assert!(t.fs.stat("/one/a.txt").is_ok());
    assert!(t.fs.stat("/one/src").is_err());
    assert_eq!(t.run("unzip z.zip src -d /two").status, 0);
    assert!(t.fs.stat("/two/src/main.rs").is_ok());
    let r = t.run("unzip z.zip a.txt -d /one");
    assert_eq!(r.status, 1);
    assert!(r.text().contains("already exists"), "{}", r.text());
    assert_eq!(t.run("unzip -o z.zip a.txt -d /one").status, 0);
    let r = t.run("unzip z.zip nothing");
    assert!(r.text().contains("no item 'nothing'"), "{}", r.text());
}

#[test]
fn unzip_refuses_names_that_escape_the_folder() {
    let mut t = project();
    // An archive with a path-traversal name, built by hand.
    let mut w = crate::format::zip::ZipWriter::new();
    w.add_file("../evil.txt", b"x", (0, 0x21)).unwrap();
    w.add_file("ok.txt", b"fine", (0, 0x21)).unwrap();
    t.fs.write("/w/evil.zip", &w.finish()).unwrap();
    let r = t.run("unzip evil.zip -d /out");
    assert_eq!(r.status, 1);
    assert!(r.text().contains("unsafe name"), "{}", r.text());
    assert!(t.fs.stat("/evil.txt").is_err() && t.fs.stat("/out/evil.txt").is_err());
    assert_eq!(t.out("cat /out/ok.txt"), "fine");
}

#[test]
fn unzip_says_what_is_wrong() {
    let mut t = project();
    let r = t.run("unzip a.txt");
    assert!(r.text().contains("not a ZIP"), "{}", r.text());
    assert_eq!(t.run("unzip").status, 2);
    assert_eq!(t.run("zip only").status, 2);
    assert_eq!(t.run("unzip missing.zip").status, 1);
}

#[test]
fn portuguese_messages() {
    let mut t = project();
    t.lang(Lang::Pt);
    assert!(t.run("gunzip a.txt").text().contains("sufixo desconhecido"));
    assert!(t.run("unzip a.txt").text().contains("não é um arquivo ZIP"));
}

#[test]
fn iconv_converts_between_encodings() {
    let mut t = project();
    t.fs.write("/w/l1.txt", b"a\xe7\xe3o\n").unwrap();
    assert_eq!(t.out("iconv -f latin1 -t utf-8 l1.txt"), "ação\n");
    t.run("iconv -f latin1 -t utf-8 l1.txt > u.txt");
    assert_eq!(t.fs.read("/w/u.txt").unwrap(), "ação\n".as_bytes());
    t.run("iconv -f utf-8 -t utf-16le u.txt > w.txt");
    assert_eq!(t.fs.read("/w/w.txt").unwrap(), b"a\0\xe7\0\xe3\0o\0\n\0");
    t.fs.write("/w/cjk", "日本".as_bytes()).unwrap();
    let r = t.run("iconv -f utf-8 -t cp1252 cjk");
    assert_eq!(r.status, 1);
    assert!(r.text().contains("cannot write"), "{}", r.text());
    assert!(
        t.run("iconv -f klingon -t utf-8 cjk")
            .text()
            .contains("unknown encoding")
    );
    assert_eq!(t.run("iconv -f utf-8").status, 2);
}
