use super::*;
use crate::format::png;
use crate::platform::appfs::MemFs;
use crate::platform::appmanifest::{ICON_SECTION, MANIFEST_SECTION, ManifestError};
use crate::platform::wasmsec;

fn leb(v: &mut Vec<u8>, mut n: u32) {
    loop {
        let b = (n & 0x7F) as u8;
        n >>= 7;
        if n == 0 {
            v.push(b);
            return;
        }
        v.push(b | 0x80);
    }
}

fn section(name: &str, data: &[u8]) -> Vec<u8> {
    let mut p = Vec::new();
    leb(&mut p, name.len() as u32);
    p.extend_from_slice(name.as_bytes());
    p.extend_from_slice(data);
    let mut s = alloc::vec![0u8];
    leb(&mut s, p.len() as u32);
    s.extend_from_slice(&p);
    s
}

fn pkg(manifest: &str) -> Vec<u8> {
    let mut w = wasmsec::HEADER.to_vec();
    w.extend_from_slice(&section(MANIFEST_SECTION, manifest.as_bytes()));
    w
}

fn app(id: &str, name: &str) -> Vec<u8> {
    pkg(&alloc::format!("id={id}\nname={name}\nversion=1.0.0\n"))
}

fn icon_png(side: usize, color: u32) -> Vec<u8> {
    let img = Image::from_pixels(side, side, alloc::vec![color; side * side]).unwrap();
    png::encode(&img).unwrap()
}

fn fs() -> MemFs {
    MemFs::new(16 << 20)
}

#[test]
fn install_then_list_then_read_back() {
    let mut f = fs();
    let w = app("clock", "Clock");
    let m = install(&mut f, &w).unwrap();
    assert_eq!(m.id, "clock");
    assert!(is_installed(&mut f, "clock"));
    assert_eq!(installed_ids(&mut f).unwrap(), ["clock"]);
    assert_eq!(read_package(&mut f, "clock").unwrap(), w);
}

#[test]
fn duplicate_id_is_refused_and_original_kept() {
    let mut f = fs();
    let a = app("dup", "First");
    install(&mut f, &a).unwrap();
    let b = app("dup", "Second");
    assert_eq!(install(&mut f, &b).unwrap_err(), InstallError::Duplicate);
    assert_eq!(read_package(&mut f, "dup").unwrap(), a);
}

#[test]
fn invalid_manifest_is_refused_without_writing() {
    let mut f = fs();
    let bad = pkg("id=x\nname=X\nversion=1.0.0\nfs=everything\n");
    assert_eq!(
        install(&mut f, &bad).unwrap_err(),
        InstallError::Package(PackageError::Manifest(ManifestError::BadValue("fs")))
    );
    assert!(installed_ids(&mut f).unwrap().is_empty());
    assert!(f.stat("/apps/.install.tmp").is_err());
}

#[test]
fn quota_above_the_ceiling_is_refused() {
    let mut f = fs();
    let bad = pkg("id=big\nname=Big\nversion=1.0.0\nmem_mib=512\n");
    assert_eq!(
        install(&mut f, &bad).unwrap_err(),
        InstallError::Package(PackageError::Manifest(ManifestError::OverLimit("mem_mib")))
    );
    let bad = pkg("id=big\nname=Big\nversion=1.0.0\nfuel_frame=999999999\n");
    assert!(matches!(
        install(&mut f, &bad),
        Err(InstallError::Package(PackageError::Manifest(
            ManifestError::OverLimit("fuel_frame")
        )))
    ));
}

#[test]
fn garbage_and_missing_manifest() {
    let mut f = fs();
    assert!(matches!(
        install(&mut f, b"hello"),
        Err(InstallError::Package(PackageError::Wasm(_)))
    ));
    assert_eq!(
        install(&mut f, &wasmsec::HEADER).unwrap_err(),
        InstallError::Package(PackageError::NoManifest)
    );
    assert!(installed_ids(&mut f).unwrap().is_empty());
}

#[test]
fn oversize_package() {
    let mut f = fs();
    let mut w = app("huge", "Huge");
    // a data-ish padding custom section pushing it over the limit
    w.extend_from_slice(&section("pad", &alloc::vec![0u8; MAX_PACKAGE_BYTES]));
    assert_eq!(install(&mut f, &w).unwrap_err(), InstallError::TooLarge);
}

#[test]
fn remove_and_reinstall() {
    let mut f = fs();
    install(&mut f, &app("gone", "Gone")).unwrap();
    remove(&mut f, "gone").unwrap();
    assert!(!is_installed(&mut f, "gone"));
    assert_eq!(
        remove(&mut f, "gone").unwrap_err(),
        InstallError::NotInstalled
    );
    assert_eq!(remove(&mut f, "../x").unwrap_err(), InstallError::BadId);
    assert_eq!(remove(&mut f, "").unwrap_err(), InstallError::BadId);
    install(&mut f, &app("gone", "Gone")).unwrap();
}

#[test]
fn remove_keeps_app_data() {
    let mut f = fs();
    install(&mut f, &app("keep", "Keep")).unwrap();
    f.mkdir_all("/data/keep").unwrap();
    f.create("/data/keep/notes").unwrap();
    remove(&mut f, "keep").unwrap();
    assert!(f.stat("/data/keep/notes").is_ok());
}

#[test]
fn max_id_length_installs() {
    let mut f = fs();
    let id = "a".repeat(32);
    install(&mut f, &app(&id, "Long")).unwrap();
    assert!(is_installed(&mut f, &id));
    assert_eq!(installed_ids(&mut f).unwrap(), std::slice::from_ref(&id));
    remove(&mut f, &id).unwrap();
}

#[test]
fn catalog_lists_name_sorted_with_icons() {
    let mut f = fs();
    install(&mut f, &app("zeta", "Zeta")).unwrap();
    let mut with_icon = app("alpha", "alpha");
    with_icon.extend_from_slice(&section(ICON_SECTION, &icon_png(48, 0xFF112233)));
    install(&mut f, &with_icon).unwrap();
    install(&mut f, &app("mid", "Mid")).unwrap();
    let cat = load_catalog(&mut f);
    let names: Vec<_> = cat.iter().map(|c| c.manifest.name.as_str()).collect();
    assert_eq!(names, ["alpha", "Mid", "Zeta"]);
    let icon = cat[0].icon.as_ref().unwrap();
    assert_eq!(icon.len(), ICON_SIZE * ICON_SIZE);
    assert_eq!(
        icon[0], 0xFF112233,
        "a constant icon scales to the same color"
    );
    assert!(cat[1].icon.is_none());
}

#[test]
fn catalog_skips_corrupt_or_mismatched_files() {
    let mut f = fs();
    install(&mut f, &app("ok", "Ok")).unwrap();
    // a file dropped into /apps by hand: wrong id for its name, and garbage
    f.create("/apps/liar.wasm").unwrap();
    f.write_at("/apps/liar.wasm", 0, &app("other", "Other"))
        .unwrap();
    f.create("/apps/junk.wasm").unwrap();
    f.write_at("/apps/junk.wasm", 0, b"not wasm").unwrap();
    f.create("/apps/readme.txt").unwrap();
    f.mkdir("/apps/dir.wasm").unwrap();
    let cat = load_catalog(&mut f);
    assert_eq!(cat.len(), 1);
    assert_eq!(cat[0].manifest.id, "ok");
    let ids = installed_ids(&mut f).unwrap();
    assert_eq!(ids, ["junk", "liar", "ok"]);
}

#[test]
fn catalog_of_a_fresh_system_is_empty() {
    let mut f = fs();
    assert!(load_catalog(&mut f).is_empty());
    assert!(installed_ids(&mut f).unwrap().is_empty());
}

#[test]
fn seed_installs_missing_and_never_overwrites() {
    let mut f = fs();
    let a = app("one", "One");
    let b = app("two", "Two");
    assert_eq!(seed(&mut f, &[&a, &b]), 2);
    // the user replaced `one` by their own build; reseeding keeps it
    remove(&mut f, "one").unwrap();
    let mine = app("one", "My One");
    install(&mut f, &mine).unwrap();
    assert_eq!(seed(&mut f, &[&a, &b]), 0);
    assert_eq!(read_package(&mut f, "one").unwrap(), mine);
    // an invalid bundled package is skipped, the rest installs
    let c = app("three", "Three");
    assert_eq!(seed(&mut f, &[b"junk", &c]), 1);
}

#[test]
fn catalog_limit() {
    let mut f = fs();
    for i in 0..MAX_APPS {
        install(&mut f, &app(&alloc::format!("a{i}"), "A")).unwrap();
    }
    assert_eq!(
        install(&mut f, &app("onetoomany", "X")).unwrap_err(),
        InstallError::Full
    );
}

#[test]
fn leftover_temp_file_is_cleaned_by_the_next_install() {
    let mut f = fs();
    f.mkdir_all("/apps").unwrap();
    f.create("/apps/.install.tmp").unwrap();
    f.write_at("/apps/.install.tmp", 0, b"stale").unwrap();
    install(&mut f, &app("fresh", "Fresh")).unwrap();
    assert!(f.stat("/apps/.install.tmp").is_err());
    assert_eq!(installed_ids(&mut f).unwrap(), ["fresh"]);
}

#[test]
fn failed_write_leaves_nothing_behind() {
    let mut f = MemFs::new(40); // far too small for a package
    let w = app("tiny", "Tiny");
    assert!(matches!(install(&mut f, &w), Err(InstallError::Fs(_))));
    assert!(installed_ids(&mut f).unwrap().is_empty());
    assert!(f.stat("/apps/.install.tmp").is_err());
}

#[test]
fn read_package_errors() {
    let mut f = fs();
    assert_eq!(
        read_package(&mut f, "nope").unwrap_err(),
        InstallError::NotInstalled
    );
    assert_eq!(
        read_package(&mut f, "../etc").unwrap_err(),
        InstallError::BadId
    );
}

#[test]
fn error_messages() {
    use alloc::string::ToString;
    for e in [
        InstallError::TooLarge,
        InstallError::Duplicate,
        InstallError::NotInstalled,
        InstallError::BadId,
        InstallError::Full,
        InstallError::Fs(FsError::NoSpace),
        InstallError::Package(PackageError::NoManifest),
    ] {
        assert!(!e.to_string().is_empty());
    }
}

#[test]
fn seed_once_offers_each_package_exactly_once() {
    let mut f = fs();
    let (a, b) = (app("alpha", "Alpha"), app("beta", "Beta"));
    let bundled: [&[u8]; 2] = [&a, &b];
    assert_eq!(seed_once(&mut f, &bundled), 2);
    assert_eq!(installed_ids(&mut f).unwrap(), ["alpha", "beta"]);
    // A second boot installs nothing.
    assert_eq!(seed_once(&mut f, &bundled), 0);
    // The user removes one: it must NOT come back at the next boot.
    remove(&mut f, "alpha").unwrap();
    assert_eq!(seed_once(&mut f, &bundled), 0);
    assert_eq!(installed_ids(&mut f).unwrap(), ["beta"]);
    // The stateless seed would have resurrected it.
    assert_eq!(seed(&mut f, &bundled), 1);
}

#[test]
fn seed_once_installs_packages_that_are_new_in_a_later_build() {
    let mut f = fs();
    let (a, b) = (app("alpha", "Alpha"), app("beta", "Beta"));
    assert_eq!(seed_once(&mut f, &[&a]), 1);
    remove(&mut f, "alpha").unwrap();
    // The OS was updated and now bundles `beta` too.
    assert_eq!(seed_once(&mut f, &[&a, &b]), 1);
    assert_eq!(installed_ids(&mut f).unwrap(), ["beta"]);
}

#[test]
fn seed_once_counts_a_user_installed_package_as_offered() {
    let mut f = fs();
    let a = app("alpha", "Alpha");
    install(&mut f, &a).unwrap();
    assert_eq!(seed_once(&mut f, &[&a]), 0);
    remove(&mut f, "alpha").unwrap();
    assert_eq!(
        seed_once(&mut f, &[&a]),
        0,
        "removal after that stays removed"
    );
}

#[test]
fn seed_once_retries_what_failed_and_ignores_a_bad_marker() {
    // No room: nothing installed, nothing marked.
    let mut tiny = MemFs::new(8);
    let a = app("alpha", "Alpha");
    assert_eq!(seed_once(&mut tiny, &[&a]), 0);
    assert!(tiny.stat("/apps/.seeded").is_err());
    // A marker full of junk (hostile ids, too long, binary) offers nothing it should not.
    let mut f = fs();
    f.mkdir_all("/apps").unwrap();
    f.create("/apps/.seeded").unwrap();
    f.write_at(
        "/apps/.seeded",
        0,
        b"../../etc\n\xff\xfe\nALPHA\nbeta\nbeta\n",
    )
    .unwrap();
    let b = app("beta", "Beta");
    assert_eq!(seed_once(&mut f, &[&a, &b]), 1);
    assert_eq!(installed_ids(&mut f).unwrap(), ["alpha"]);
}

#[test]
fn the_seed_marker_is_not_an_app_and_survives_catalog_walks() {
    let mut f = fs();
    let a = app("alpha", "Alpha");
    seed_once(&mut f, &[&a]);
    assert_eq!(installed_ids(&mut f).unwrap(), ["alpha"]);
    assert_eq!(load_catalog(&mut f).len(), 1);
    assert_eq!(f.stat("/apps/.seeded").unwrap().kind, Kind::File);
}

#[test]
fn the_catalog_sorts_by_the_name_of_the_language() {
    let mut f = fs();
    install(
        &mut f,
        &pkg("id=clock\nname=Clock\nname.pt=Relógio\nversion=1.0.0\n"),
    )
    .unwrap();
    install(
        &mut f,
        &pkg("id=notes\nname=Notes\nname.pt=Notas\nversion=1.0.0\n"),
    )
    .unwrap();
    install(
        &mut f,
        &pkg("id=paint\nname=Paint\nname.pt=Pintura\nversion=1.0.0\n"),
    )
    .unwrap();
    let mut ids = |l| -> Vec<String> {
        load_catalog_in(&mut f, l)
            .iter()
            .map(|c| c.manifest.id.clone())
            .collect()
    };
    assert_eq!(ids(Lang::Pt), ["notes", "paint", "clock"]);
    assert_eq!(ids(Lang::En), ["clock", "notes", "paint"]);
}

#[test]
fn install_errors_read_in_both_languages() {
    let errs = [
        InstallError::TooLarge,
        InstallError::Duplicate,
        InstallError::NotInstalled,
        InstallError::BadId,
        InstallError::Full,
        InstallError::Fs(FsError::NoSpace),
        InstallError::Package(PackageError::NoManifest),
        InstallError::Package(PackageError::Manifest(ManifestError::Missing("name"))),
    ];
    for e in &errs {
        let (pt, en) = (e.message_in(Lang::Pt), e.message_in(Lang::En));
        assert!(!pt.is_empty() && !en.is_empty() && pt != en, "{e:?}");
        assert!(!pt.contains("apps.") && !en.contains("apps."), "{e:?}");
        assert!(!pt.contains('{') && !en.contains('{'), "{e:?}");
    }
    assert_eq!(
        InstallError::Duplicate.message_in(Lang::Pt),
        "Já existe um app instalado com este identificador"
    );
    assert_eq!(
        InstallError::Fs(FsError::NoSpace).message_in(Lang::En),
        "File system error: no space left (quota)"
    );
    assert_eq!(
        InstallError::Fs(FsError::NoSpace).message_in(Lang::Pt),
        "Erro no sistema de arquivos: sem espaço (cota)"
    );
    // The log text stays English.
    assert_eq!(
        alloc::format!("{}", InstallError::Duplicate),
        "an app with this id is already installed"
    );
}

fn app_v(id: &str, name: &str, version: &str) -> Vec<u8> {
    pkg(&alloc::format!("id={id}\nname={name}\nversion={version}\n"))
}

#[test]
fn seed_once_upgrades_an_older_installed_copy_and_keeps_a_newer_one() {
    let mut f = fs();
    let v1 = app_v("alpha", "Alpha", "1.0.0");
    assert_eq!(seed_once(&mut f, &[&v1]), 1);
    // The next build bundles 1.1.0 under a better name: the installed copy is replaced.
    let v2 = app_v("alpha", "Alfa", "1.1.0");
    assert_eq!(seed_once(&mut f, &[&v2]), 1);
    assert_eq!(read_package(&mut f, "alpha").unwrap(), v2);
    // Same version again: nothing to do.
    assert_eq!(seed_once(&mut f, &[&v2]), 0);
    // An older bundle never downgrades what is installed.
    assert_eq!(seed_once(&mut f, &[&v1]), 0);
    assert_eq!(read_package(&mut f, "alpha").unwrap(), v2);
}

#[test]
fn seed_once_does_not_resurrect_a_removed_app_when_upgrading() {
    let mut f = fs();
    let v1 = app_v("alpha", "Alpha", "1.0.0");
    assert_eq!(seed_once(&mut f, &[&v1]), 1);
    remove(&mut f, "alpha").unwrap();
    let v2 = app_v("alpha", "Alfa", "1.1.0");
    assert_eq!(seed_once(&mut f, &[&v2]), 0);
    assert!(!is_installed(&mut f, "alpha"));
}

// ---- read_package_meta: the catalog reads sections, not whole packages ----

fn put(fs: &mut MemFs, id: &str, bytes: &[u8]) {
    fs.mkdir_all(APPS_DIR).unwrap();
    let path = package_path(id);
    fs.create(&path).unwrap();
    write_all(fs, &path, bytes).unwrap();
}

/// Same module with a fat code section and an unrelated custom section around the metadata.
fn padded(manifest: &str, icon: Option<&[u8]>) -> Vec<u8> {
    let mut w = wasmsec::HEADER.to_vec();
    w.extend_from_slice(&section("name", &[7u8; 5000]));
    // a type section (id 1) and a code section (id 10) with some payload
    for id in [1u8, 10] {
        let mut s = alloc::vec![id];
        leb(&mut s, 3000);
        s.extend_from_slice(&[0xAA; 3000]);
        w.extend_from_slice(&s);
    }
    w.extend_from_slice(&section(MANIFEST_SECTION, manifest.as_bytes()));
    if let Some(i) = icon {
        w.extend_from_slice(&section(ICON_SECTION, i));
    }
    w.extend_from_slice(&section("producers", &[1u8; 700]));
    w
}

fn same_answer(bytes: &[u8]) {
    let mut fs = MemFs::new(1 << 20);
    put(&mut fs, "x", bytes);
    let whole = appmanifest::parse_package(bytes).map(|p| (p.manifest, p.icon.is_some()));
    let meta = read_package_meta(&mut fs, "x")
        .map_err(|e| match e {
            InstallError::Package(p) => p,
            other => panic!("unexpected {other:?}"),
        })
        .and_then(|m| appmanifest::parse_package(&m))
        .map(|p| (p.manifest, p.icon.is_some()));
    assert_eq!(format!("{whole:?}"), format!("{meta:?}"));
}

#[test]
fn meta_reads_give_the_same_answer_as_reading_the_whole_package() {
    let m = "id=x\nname=X\nversion=1.0.0\n";
    same_answer(&pkg(m));
    same_answer(&padded(m, None));
    same_answer(&padded(m, Some(&icon_png(24, 0xFF3366CC))));
    // no manifest at all
    let mut none = wasmsec::HEADER.to_vec();
    none.extend_from_slice(&section("name", &[1; 40]));
    same_answer(&none);
    // two manifests, a bad manifest, the legacy section names
    let mut two = pkg(m);
    two.extend_from_slice(&section(MANIFEST_SECTION, m.as_bytes()));
    same_answer(&two);
    same_answer(&pkg("id=Bad Id\nname=X\nversion=1\n"));
    let mut legacy = wasmsec::HEADER.to_vec();
    legacy.extend_from_slice(&section("osjeff.manifest", m.as_bytes()));
    same_answer(&legacy);
}

#[test]
fn meta_reads_reject_what_the_section_walk_rejects() {
    let m = "id=x\nname=X\nversion=1.0.0\n";
    let good = padded(m, None);
    // truncated anywhere inside the file
    for cut in [3, 8, 20, 6000, good.len() - 1] {
        same_answer(&good[..cut]);
    }
    // bad magic, bad version, an unknown section id
    let mut bad = good.clone();
    bad[1] = b'x';
    same_answer(&bad);
    let mut bad = good.clone();
    bad[4] = 2;
    same_answer(&bad);
    let mut bad = wasmsec::HEADER.to_vec();
    bad.extend_from_slice(&[14, 0]);
    same_answer(&bad);
    // a custom section whose name does not fit in its payload
    let mut bad = wasmsec::HEADER.to_vec();
    bad.extend_from_slice(&[0, 2, 9, b'a']);
    same_answer(&bad);
}

#[test]
fn meta_reads_never_pull_in_the_code() {
    struct Counting {
        inner: MemFs,
        read: usize,
    }
    impl AppFs for Counting {
        fn stat(&mut self, p: &str) -> Result<crate::platform::appfs::Stat, FsError> {
            self.inner.stat(p)
        }
        fn read_at(&mut self, p: &str, off: u64, buf: &mut [u8]) -> Result<usize, FsError> {
            let n = self.inner.read_at(p, off, buf)?;
            self.read += n;
            Ok(n)
        }
        fn write_at(&mut self, p: &str, off: u64, d: &[u8]) -> Result<usize, FsError> {
            self.inner.write_at(p, off, d)
        }
        fn set_len(&mut self, p: &str, len: u64) -> Result<(), FsError> {
            self.inner.set_len(p, len)
        }
        fn create(&mut self, p: &str) -> Result<(), FsError> {
            self.inner.create(p)
        }
        fn mkdir(&mut self, p: &str) -> Result<(), FsError> {
            self.inner.mkdir(p)
        }
        fn remove(&mut self, p: &str) -> Result<(), FsError> {
            self.inner.remove(p)
        }
        fn rename(&mut self, a: &str, b: &str) -> Result<(), FsError> {
            self.inner.rename(a, b)
        }
        fn read_dir(
            &mut self,
            p: &str,
            i: usize,
        ) -> Result<Option<crate::platform::appfs::DirEntry>, FsError> {
            self.inner.read_dir(p, i)
        }
        fn tree_size(&mut self, p: &str) -> Result<u64, FsError> {
            self.inner.tree_size(p)
        }
    }
    let m = "id=x\nname=X\nversion=1.0.0\n";
    let bytes = padded(m, Some(&icon_png(24, 0xFF112233)));
    let mut fs = Counting {
        inner: MemFs::new(1 << 20),
        read: 0,
    };
    put(&mut fs.inner, "x", &bytes);
    let meta = read_package_meta(&mut fs, "x").unwrap();
    assert!(appmanifest::parse_package(&meta).unwrap().icon.is_some());
    // The 11 KB of padding is never read; only headers and the two sections are.
    assert!(
        fs.read < bytes.len() / 2,
        "read {} of {}",
        fs.read,
        bytes.len()
    );
    assert!(meta.len() < bytes.len() / 2);
}
