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

#[test]
fn run_hands_over_the_arguments_the_folder_and_the_piped_input() {
    let mut t = with_programs();
    assert_eq!(t.run("run hello one 'two words' three").status, 0);
    let got = &t.sys.started_with[0];
    assert_eq!(got.args, ["one", "two words", "three"]);
    assert_eq!(got.cwd, "/");
    assert!(got.stdin.is_empty());
    assert_eq!(t.run("cd /bin").status, 0);
    assert_eq!(t.run("echo piped | run hello -x").status, 0);
    let got = &t.sys.started_with[1];
    assert_eq!(
        (got.cwd.as_str(), got.args.as_slice()),
        ("/bin", &["-x".to_string()][..])
    );
    assert_eq!(got.stdin, b"piped\n");
}

#[test]
fn sound_commands_run_the_play_program() {
    let mut t = with_programs();
    t.sys.programs.push(("play".into(), Vec::new(), Some(0)));
    // A file called `play` in the folder does not take the place of the program.
    t.fs = MemFs::new().with_file("/play", b"\x7fELF fake");
    assert_eq!(t.run("play som.wav -v 40").status, 0);
    assert_eq!(t.run("tone 440").status, 0);
    assert_eq!(t.run("tone 440,660 2000 -v 50").status, 0);
    assert_eq!(t.run("beep").status, 0);
    assert_eq!(t.run("volume 30").status, 0);
    assert_eq!(t.sys.started, ["play"; 5]);
    let args: Vec<Vec<String>> = t.sys.started_with.iter().map(|c| c.args.clone()).collect();
    assert_eq!(args[0], ["som.wav", "-v", "40"]);
    assert_eq!(args[1], ["--tone", "440", "1000"]);
    assert_eq!(args[2], ["--tone", "440,660", "2000", "-v", "50"]);
    assert_eq!(args[3], ["--tone", "880", "150"]);
    assert_eq!(args[4], ["--volume", "30"]);
    assert_eq!(t.run("play").status, 2);
    assert_eq!(t.run("tone").status, 2);
}
