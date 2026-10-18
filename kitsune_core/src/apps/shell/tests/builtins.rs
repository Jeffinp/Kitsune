use super::*;

#[test]
fn pwd_and_cd() {
    let mut t = T::with_files();
    assert_eq!(t.out("pwd"), "/\n");
    assert_eq!(t.out("cd d; pwd"), "/d\n");
    assert_eq!(t.out("cd ..; pwd"), "/\n");
    assert_eq!(t.out("cd /d; cd -; pwd"), "/\n/\n");
    assert_eq!(t.out("cd /d && echo $PWD $OLDPWD"), "/d /\n");
    let r = t.run("cd nope");
    assert_eq!(r.status, 1);
    assert!(r.text().contains("No such file"));
    assert_eq!(t.run("cd a.txt").status, 1);
    t.sh.env.set("HOME", "/d");
    assert_eq!(t.out("cd; pwd"), "/d\n");
}

#[test]
fn ls_variants() {
    let mut t = T::with_files();
    t.fs.write("/.hid", b"").unwrap();
    assert_eq!(t.out("ls"), "a.txt\nb.txt\nc.md\nd\n");
    assert_eq!(t.out("ls -F"), "a.txt\nb.txt\nc.md\nd/\n");
    assert!(t.out("ls -a").starts_with(".\n..\n.hid\n"));
    assert_eq!(t.out("ls d"), "x.txt\n");
    assert_eq!(t.out("ls a.txt"), "a.txt\n");
    assert_eq!(t.out("ls -l d"), "-        9 x.txt\n");
    assert_eq!(t.out("ls d /d"), "d:\nx.txt\n\n/d:\nx.txt\n");
    let r = t.run("ls /nope");
    assert_eq!(r.status, 2);
    assert!(r.text().contains("cannot access '/nope'"));
    assert_eq!(t.run("ls -Z").status, 2);
}

#[test]
fn cat_variants() {
    let mut t = T::with_files();
    assert_eq!(t.out("cat a.txt b.txt"), "alpha\nbeta\ngamma\n");
    assert_eq!(t.out("cat -n a.txt"), "     1\talpha\n     2\tbeta\n");
    assert_eq!(t.out("echo hi | cat - a.txt"), "hi\nalpha\nbeta\n");
    let r = t.run("cat nope a.txt");
    assert_eq!(r.status, 1);
    assert!(r.text().contains("nope: No such file"));
    assert!(r.text().contains("alpha"));
    assert_eq!(t.run("cat d").status, 1);
}

#[test]
fn mkdir_rm_rmdir() {
    let mut t = T::new();
    assert_eq!(t.run("mkdir a").status, 0);
    assert_eq!(t.run("mkdir a").status, 1);
    assert_eq!(t.run("mkdir -p x/y/z").status, 0);
    assert!(t.fs.stat("/x/y/z").is_ok());
    assert_eq!(t.run("mkdir -p x/y/z").status, 0);
    t.run("touch x/f");
    assert_eq!(t.run("mkdir -p x/f/g").status, 1);
    assert_eq!(t.run("rmdir x").status, 1);
    assert_eq!(t.run("rm x").status, 1);
    assert_eq!(t.run("rm -r x").status, 0);
    assert!(t.fs.stat("/x").is_err());
    assert_eq!(t.run("rm nope").status, 1);
    assert_eq!(t.run("rm -f nope").status, 0);
    assert_eq!(t.run("rmdir a").status, 0);
    assert_eq!(t.run("rmdir nope").status, 1);
    assert_eq!(t.run("mkdir").status, 2);
}

#[test]
fn rm_refuses_dangerous_targets() {
    let mut t = T::with_files();
    assert_eq!(t.run("rm -rf /").status, 1);
    assert_eq!(t.run("rm -rf .").status, 1);
    assert_eq!(t.run("rm -rf ..").status, 1);
    assert!(t.fs.stat("/a.txt").is_ok());
}

#[test]
fn mv_and_cp() {
    let mut t = T::with_files();
    assert_eq!(t.run("mv a.txt z.txt").status, 0);
    assert!(t.fs.stat("/a.txt").is_err());
    assert_eq!(t.run("mv z.txt d").status, 0);
    assert!(t.fs.stat("/d/z.txt").is_ok());
    assert_eq!(t.run("mv b.txt c.md d").status, 0);
    assert!(t.fs.stat("/d/c.md").is_ok());
    assert_eq!(t.run("mv nope d").status, 1);
    assert_eq!(t.run("mv").status, 2);
    assert_eq!(t.run("cp d/x.txt copy.txt").status, 0);
    assert_eq!(t.fs.read("/copy.txt").unwrap(), b"x1\nx2\nx3\n");
    assert_eq!(t.run("cp d dd").status, 1);
    assert_eq!(t.run("cp -r d dd").status, 0);
    assert!(t.fs.stat("/dd/x.txt").is_ok());
    assert_eq!(t.run("cp copy.txt d").status, 0);
    assert!(t.fs.stat("/d/copy.txt").is_ok());
    assert_eq!(t.run("cp -r d d/inside").status, 1);
}

#[test]
fn touch_creates_but_keeps_content() {
    let mut t = T::with_files();
    assert_eq!(t.run("touch new a.txt").status, 0);
    assert_eq!(t.fs.read("/new").unwrap(), b"");
    assert_eq!(t.fs.read("/a.txt").unwrap(), b"alpha\nbeta\n");
    assert_eq!(t.run("touch /no/dir/f").status, 1);
}

#[test]
fn head_and_tail() {
    let mut t = T::with_files();
    assert_eq!(t.out("head -n 2 d/x.txt"), "x1\nx2\n");
    assert_eq!(t.out("head -1 d/x.txt"), "x1\n");
    assert_eq!(t.out("tail -n 1 d/x.txt"), "x3\n");
    assert_eq!(t.out("tail -2 d/x.txt"), "x2\nx3\n");
    assert_eq!(t.out("seq 20 | head -n 3"), "1\n2\n3\n");
    assert_eq!(t.out("seq 20 | tail -n 2"), "19\n20\n");
    assert_eq!(t.out("seq 20 | head | wc -l"), "10\n");
    assert_eq!(
        t.out("head -n 1 a.txt b.txt"),
        "==> a.txt <==\nalpha\n\n==> b.txt <==\ngamma\n"
    );
    assert_eq!(t.run("head -n x a.txt").status, 2);
    assert_eq!(t.out("head -n 0 a.txt"), "");
}

#[test]
fn wc_counts() {
    let mut t = T::with_files();
    assert_eq!(t.out("wc a.txt"), "2 2 11 a.txt\n");
    assert_eq!(t.out("wc -l a.txt"), "2 a.txt\n");
    assert_eq!(t.out("wc -w < a.txt"), "2\n");
    assert_eq!(t.out("echo héllo | wc -m"), "6\n");
    assert_eq!(t.out("echo héllo | wc -c"), "7\n");
    assert_eq!(t.out("wc -l a.txt b.txt"), "2 a.txt\n1 b.txt\n3 total\n");
    assert_eq!(t.run("wc nope").status, 1);
}

#[test]
fn grep_variants() {
    let mut t = T::with_files();
    assert_eq!(t.out("grep a a.txt"), "alpha\nbeta\n");
    assert_eq!(t.out("grep ^b a.txt"), "beta\n");
    assert_eq!(t.out("grep -n eta a.txt"), "2:beta\n");
    assert_eq!(t.out("grep -c a a.txt"), "2\n");
    assert_eq!(t.out("grep -v alpha a.txt"), "beta\n");
    assert_eq!(t.out("grep -i ALPHA a.txt"), "alpha\n");
    assert_eq!(t.out("grep -l a a.txt b.txt c.md"), "a.txt\nb.txt\n");
    assert_eq!(
        t.out("grep a a.txt b.txt"),
        "a.txt:alpha\na.txt:beta\nb.txt:gamma\n"
    );
    assert_eq!(t.out("grep -h a a.txt b.txt").lines().count(), 3);
    assert_eq!(t.out("echo 'a.b' | grep -F 'a.b'"), "a.b\n");
    assert_eq!(t.out("echo axb | grep -F 'a.b'"), "");
    assert_eq!(t.out("cat a.txt | grep -E 'alp|bet'"), "alpha\nbeta\n");
    assert_eq!(t.run("grep zzz a.txt").status, 1);
    assert_eq!(t.run("grep -q alpha a.txt").status, 0);
    assert_eq!(t.out("grep -q alpha a.txt"), "");
    assert_eq!(t.run("grep '(' a.txt").status, 2);
    assert_eq!(t.run("grep").status, 2);
    assert_eq!(t.run("grep a nope").status, 2);
}

#[test]
fn sort_variants() {
    let mut t = T::new();
    assert_eq!(t.out("echo -e 'b\\nc\\na' | sort"), "a\nb\nc\n");
    assert_eq!(t.out("echo -e 'b\\nc\\na' | sort -r"), "c\nb\na\n");
    assert_eq!(t.out("echo -e '10\\n9\\n100' | sort -n"), "9\n10\n100\n");
    assert_eq!(t.out("echo -e '10\\n9\\n100' | sort"), "10\n100\n9\n");
    assert_eq!(t.out("echo -e 'a\\na\\nb' | sort -u"), "a\nb\n");
    assert_eq!(t.out("echo -e 'B\\na' | sort -f"), "a\nB\n");
    assert_eq!(t.out("echo -e '-5\\n3\\n-10' | sort -n"), "-10\n-5\n3\n");
}

#[test]
fn uniq_variants() {
    let mut t = T::new();
    let data = "echo -e 'a\\na\\nb\\na'";
    assert_eq!(t.out(&format!("{data} | uniq")), "a\nb\na\n");
    assert_eq!(
        t.out(&format!("{data} | uniq -c")),
        "      2 a\n      1 b\n      1 a\n"
    );
    assert_eq!(t.out(&format!("{data} | uniq -d")), "a\n");
    assert_eq!(t.out(&format!("{data} | uniq -u")), "b\na\n");
    assert_eq!(
        t.out(&format!("{data} | sort | uniq -c")),
        "      3 a\n      1 b\n"
    );
}

#[test]
fn tee_copies() {
    let mut t = T::new();
    assert_eq!(t.out("echo hi | tee out.txt"), "hi\n");
    assert_eq!(t.fs.read("/out.txt").unwrap(), b"hi\n");
    t.run("echo more | tee -a out.txt > /dev/null");
    assert_eq!(t.fs.read("/out.txt").unwrap(), b"hi\nmore\n");
}

#[test]
fn env_export_set_unset() {
    let mut t = T::new();
    t.run("export A=1 B");
    assert!(t.sh.env.is_exported("A"));
    assert!(t.sh.env.is_exported("B"));
    assert!(t.out("env").contains("A=1\n"));
    assert!(t.out("export").contains("export A=\"1\"\n"));
    t.run("C=3");
    assert!(!t.out("env").contains("C=3"));
    assert!(t.out("set").contains("C=3\n"));
    t.run("unset A");
    assert_eq!(t.sh.env.get("A"), None);
    t.run("set D=4");
    assert_eq!(t.sh.env.get("D"), Some("4"));
    assert_eq!(t.run("export 1bad=2").status, 1);
    assert_eq!(t.run("set oops").status, 2);
}

#[test]
fn set_replaces_positional_parameters() {
    let mut t = T::new();
    assert_eq!(t.script("set -- x y z; echo $# $2", &[]).text(), "3 y\n");
}

#[test]
fn history_command() {
    let mut t = T::new();
    t.run("echo one");
    t.run("echo two");
    let h = t.out("history");
    assert!(h.contains("    1  echo one\n"));
    assert!(h.contains("    3  history\n"));
    assert_eq!(t.out("history 1"), "    4  history 1\n");
    t.run("history -c");
    assert_eq!(t.sh.history().len(), 0);
    assert_eq!(t.run("history x").status, 2);
}

#[test]
fn alias_and_unalias() {
    let mut t = T::with_files();
    t.run("alias ll='ls -F'");
    assert_eq!(t.out("ll"), "a.txt\nb.txt\nc.md\nd/\n");
    assert_eq!(t.out("alias ll"), "alias ll='ls -F'\n");
    assert!(t.out("alias").contains("alias ll='ls -F'"));
    assert_eq!(t.out("which ll"), "ll: aliased to 'ls -F'\n");
    t.run("unalias ll");
    assert_eq!(t.run("ll").status, 127);
    assert_eq!(t.run("unalias ll").status, 1);
    t.run("alias a1='echo hi'");
    t.run("unalias -a");
    assert!(t.sh.aliases().is_empty());
}

#[test]
fn alias_loops_terminate() {
    let mut t = T::new();
    t.run("alias echo='echo echo'");
    let r = t.run("echo x");
    assert_eq!(r.text(), "echo x\n");
    t.run("alias a='b'");
    t.run("alias b='a'");
    assert_eq!(t.run("a").status, 127);
}

#[test]
fn which_resolution() {
    let mut t = T::new();
    assert_eq!(t.out("which ls"), "ls: shell builtin\n");
    t.run("f() { :; }");
    assert_eq!(t.out("which f"), "f: shell function\n");
    let r = t.run("which nope");
    assert_eq!(r.status, 1);
    assert!(r.text().contains("nope not found"));
}

#[test]
fn date_uptime_free_df() {
    let mut t = T::new();
    assert_eq!(t.out("date"), "2026-10-07 13:05:09\n");
    assert_eq!(t.out("date +%d/%m/%Y"), "07/10/2026\n");
    assert_eq!(t.out("date +%T"), "13:05:09\n");
    assert_eq!(t.run("date bad").status, 1);
    assert_eq!(t.out("uptime"), "up 1 day, 02:03:04\n");
    t.sys.uptime = 3_725_000;
    assert_eq!(t.out("uptime"), "up 01:02:05\n");
    let f = t.out("free");
    assert!(f.contains("KiB") && f.contains("65536") && f.contains("20480") && f.contains("45056"));
    assert!(t.out("free -m").contains("MiB"));
    let d = t.out("df");
    assert!(d.contains("Filesystem") && d.contains("rootfs"));
    t.sys.disk_list.push(super::sys::DiskInfo {
        name: "ojfs".into(),
        mount: "/".into(),
        total: 1 << 20,
        used: 1 << 19,
    });
    assert!(t.out("df").contains("50%"));
}

#[test]
fn ps_kill_ping_sleep_clear() {
    let mut t = T::new();
    let p = t.out("ps");
    assert!(p.contains("init") && p.contains("shell"));
    assert_eq!(t.run("kill 7").status, 0);
    assert_eq!(t.sys.killed, [(7, 15)]);
    assert_eq!(t.run("kill -9 1").status, 0);
    assert_eq!(t.sys.killed[1], (1, 9));
    assert_eq!(t.run("kill -KILL 99").status, 1);
    assert_eq!(t.run("kill -BOGUS 1").status, 2);
    assert_eq!(t.run("kill").status, 2);
    assert_eq!(t.run("kill abc").status, 1);
    let r = t.run("ping -c 2 example.org");
    assert!(
        r.text()
            .contains("2 packets transmitted, 2 received, 0% packet loss")
    );
    assert!(r.text().contains("avg rtt = 1.500 ms"));
    t.sys.ping_ok = false;
    assert_eq!(t.run("ping host").status, 1);
    assert_eq!(t.run("ping -c 0 host").status, 2);
    assert_eq!(t.run("ping").status, 2);
    t.run("sleep 2");
    t.run("sleep 0.25");
    t.run("sleep 500000");
    assert_eq!(t.sys.slept, [2000, 250, 10_000]);
    assert_eq!(t.run("sleep abc").status, 1);
    assert_eq!(t.run("sleep").status, 2);
    let r = t.run("echo before; clear");
    assert!(r.clear);
    assert_eq!(r.text(), "");
    assert_eq!(t.sys.cleared, 1);
}

#[test]
fn unsupported_system_calls_are_reported() {
    let _lang = LangGuard::new(Lang::En);
    let mut sh = Shell::new();
    let mut fs = MemFs::new();
    let mut sys = super::sys::MinimalSys;
    let mut h = Host {
        fs: &mut fs,
        sys: &mut sys,
    };
    let r = sh.run_line("ping host", &mut h);
    assert_eq!(r.status, 1);
    assert!(r.text().contains("not supported"));
    let r = sh.run_line("kill 1", &mut h);
    assert_eq!(r.status, 1);
    assert!(sh.run_line("ps", &mut h).text().contains("PID"));
}

#[test]
fn test_builtin() {
    let mut t = T::with_files();
    let st = |t: &mut T, l: &str| t.run(l).status;
    assert_eq!(st(&mut t, "test -f a.txt"), 0);
    assert_eq!(st(&mut t, "test -d a.txt"), 1);
    assert_eq!(st(&mut t, "test -d d"), 0);
    assert_eq!(st(&mut t, "test -e nope"), 1);
    assert_eq!(st(&mut t, "test -s a.txt"), 0);
    assert_eq!(st(&mut t, "test -z ''"), 0);
    assert_eq!(st(&mut t, "test -n ''"), 1);
    assert_eq!(st(&mut t, "test a = a"), 0);
    assert_eq!(st(&mut t, "test a != a"), 1);
    assert_eq!(st(&mut t, "test 3 -lt 10"), 0);
    assert_eq!(st(&mut t, "test 3 -ge 10"), 1);
    assert_eq!(st(&mut t, "test 5 -eq 5 -a 1 -ne 2"), 0);
    assert_eq!(st(&mut t, "test 1 -eq 2 -o 2 -eq 2"), 0);
    assert_eq!(st(&mut t, "test ! -f nope"), 0);
    assert_eq!(st(&mut t, "test -f a.txt -a -d d"), 0);
    assert_eq!(st(&mut t, "test hello"), 0);
    assert_eq!(st(&mut t, "test"), 1);
    assert_eq!(st(&mut t, "test x -eq 1"), 2);
    assert_eq!(st(&mut t, "[ 1 -eq 1 ]"), 0);
    assert_eq!(st(&mut t, "[ 1 -eq 1"), 2);
    assert_eq!(st(&mut t, "[ \\( 1 -eq 1 \\) ]"), 0);
    assert_eq!(st(&mut t, "[ -n \"$UNSET\" ]"), 1);
}

#[test]
fn crash_is_for_administrators_and_names_a_service() {
    let mut t = T::new();
    let admin = |admin| crate::apps::shell::sys::Identity {
        name: "u".into(),
        uid: 1000,
        gid: 1000,
        gname: "u".into(),
        groups: Vec::new(),
        admin,
    };
    // no accounts at all: allowed
    assert_eq!(t.run("crash fetcher").status, 0);
    assert_eq!(t.sys.crashed, ["fetcher"]);
    t.sys.who = Some(admin(true));
    assert_eq!(t.run("crash appd").status, 0);
    assert_eq!(t.sys.crashed, ["fetcher", "appd"]);
    // an ordinary user is refused and nothing is crashed
    t.sys.who = Some(admin(false));
    let r = t.run("crash fetcher");
    assert_eq!(r.status, 1);
    assert_eq!(t.sys.crashed.len(), 2);
    t.sys.who = Some(admin(true));
    assert_eq!(t.run("crash nosuch").status, 1, "unknown service");
    assert_eq!(t.run("crash").status, 2);
    assert_eq!(t.run("crash a b").status, 2);
    assert!(
        t.out("help")
            .lines()
            .any(|l| l.trim_start().starts_with("crash"))
    );
}

#[test]
fn help_lists_every_command() {
    let mut t = T::new();
    let h = t.out("help");
    for cmd in [
        "help", "ls", "cd", "pwd", "cat", "echo", "mkdir", "rm", "rmdir", "mv", "cp", "touch",
        "head", "tail", "wc", "grep", "sort", "uniq", "tee", "clear", "env", "export", "set",
        "unset", "history", "alias", "which", "date", "uptime", "free", "df", "ps", "kill", "ping",
        "true", "false", "test", "sleep",
    ] {
        assert!(
            h.lines().any(|l| l.trim_start().starts_with(cmd)),
            "help is missing {cmd}"
        );
    }
    assert!(t.out("help ls").starts_with("ls "));
    assert_eq!(t.run("help nonexistent").status, 1);
}

#[test]
fn true_and_false() {
    let mut t = T::new();
    assert_eq!(t.run("true").status, 0);
    assert_eq!(t.run("false").status, 1);
    assert_eq!(t.run(":").status, 0);
}

#[test]
fn seq_basename_dirname_misc() {
    let mut t = T::new();
    assert_eq!(t.out("seq 3"), "1\n2\n3\n");
    assert_eq!(t.out("seq 2 4"), "2\n3\n4\n");
    assert_eq!(t.out("seq 10 -4 0"), "10\n6\n2\n");
    assert_eq!(t.run("seq 1 0 5").status, 2);
    assert_eq!(t.out("basename /a/b/c.txt"), "c.txt\n");
    assert_eq!(t.out("basename /a/b/c.txt .txt"), "c\n");
    assert_eq!(t.out("dirname /a/b/c.txt"), "/a/b\n");
    assert_eq!(t.out("dirname c.txt"), ".\n");
    assert_eq!(t.out("dirname /c"), "/\n");
    assert_eq!(t.out("echo abc | rev"), "cba\n");
    assert_eq!(t.out("echo hello | tr a-z A-Z"), "HELLO\n");
    assert_eq!(t.out("echo hello | tr -d l"), "heo\n");
    assert_eq!(t.out("echo a:b:c | cut -d : -f 2,3"), "b:c\n");
    assert_eq!(t.out("echo -e 'x\\ny' | nl"), "     1\tx\n     2\ty\n");
}

#[test]
fn stat_command() {
    let mut t = T::with_files();
    assert_eq!(
        t.out("stat a.txt d"),
        "a.txt: file, 11 bytes\nd: directory, 0 bytes\n"
    );
    assert_eq!(t.run("stat nope").status, 1);
}

#[test]
fn custom_commands_can_be_registered() {
    fn hello(cx: &mut CmdCtx<'_>) -> i32 {
        cx.println("custom!");
        7
    }
    let mut t = T::new();
    t.sh.register("hello", "hello: demo", hello);
    let r = t.run("hello");
    assert_eq!(r.text(), "custom!\n");
    assert_eq!(r.status, 7);
    assert_eq!(t.out("help hello"), "hello: demo\n");
}

#[test]
fn prompt_shows_cwd_and_home_tilde() {
    let mut t = T::with_files();
    assert_eq!(t.sh.prompt(&t.fs, &t.sys), "user@kitsune:/$ ");
    t.run("cd d");
    assert_eq!(t.sh.prompt(&t.fs, &t.sys), "user@kitsune:/d$ ");
    t.sh.env.set("HOME", "/d");
    assert_eq!(t.sh.prompt(&t.fs, &t.sys), "user@kitsune:~$ ");
    t.sh.env.set("PS1", "[\\W] \\\\ \\x ");
    assert_eq!(t.sh.prompt(&t.fs, &t.sys), "[d] \\ \\x ");
}

#[test]
fn history_is_recorded_by_run_line_only() {
    let mut t = T::new();
    t.run("echo a");
    t.run("echo a");
    t.run("   ");
    t.run(" hidden");
    t.script("echo scripted", &[]);
    assert_eq!(t.sh.history().len(), 1);
}
