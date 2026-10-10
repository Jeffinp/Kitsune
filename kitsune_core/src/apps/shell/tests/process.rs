//! `run`: ring 3 programs started and waited for through `SysInfo`.

use super::*;

fn with_programs() -> T {
    let mut t = T::new();
    t.sys.programs = alloc::vec![
        ("hello".into(), b"hello from ring 3\n".to_vec(), Some(0)),
        ("fails".into(), b"oops\n".to_vec(), Some(3)),
        ("crash".into(), b"about to crash\n".to_vec(), None),
    ];
    t.fs = MemFs::new()
        .with_dir("/bin")
        .with_file("/bin/prog.elf", b"\x7fELF....")
        .with_file("/plain.txt", b"not a program");
    t
}

#[test]
fn run_shows_the_output_and_returns_the_status() {
    let mut t = with_programs();
    let r = t.run("run hello");
    assert_eq!((r.status, r.text().as_str()), (0, "hello from ring 3\n"));
    let r = t.run("run fails");
    assert_eq!(r.status, 3);
    assert!(
        r.text().contains("oops") && r.text().contains("status 3"),
        "{}",
        r.text()
    );
    assert_eq!(t.sys.started, ["hello", "fails"]);
}

#[test]
fn a_program_killed_by_a_fault_says_so() {
    let mut t = with_programs();
    let r = t.run("run crash");
    assert_eq!(r.status, 139);
    assert!(
        r.text().contains("about to crash") && r.text().contains("stopped"),
        "{}",
        r.text()
    );
}

#[test]
fn run_takes_a_file_or_a_name() {
    let mut t = with_programs();
    assert_eq!(t.run("run /bin/prog.elf").status, 0);
    assert_eq!(t.sys.started, ["prog"]);
    let r = t.run("run /plain.txt");
    assert_eq!(r.status, 1);
    assert!(
        r.text().contains("not a program this system can run"),
        "{}",
        r.text()
    );
    let r = t.run("run nothing");
    assert_eq!(r.status, 1);
    assert!(r.text().contains("nothing"), "{}", r.text());
    assert_eq!(t.run("run").status, 2);
}

#[test]
fn portuguese_messages() {
    let mut t = with_programs();
    t.lang(Lang::Pt);
    assert!(t.run("run nothing").text().contains("nenhum programa"));
}
