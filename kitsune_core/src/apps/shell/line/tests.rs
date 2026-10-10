use super::*;
use crate::apps::shell::fs::MemFs;
use crate::apps::shell::{Host, MockSys};
use crate::system::input::Mods;

fn key(e: &mut LineEditor, h: &History, c: &dyn Completer, code: KeyCode, m: Mods) -> LineEvent {
    e.handle_key(KeyEvent::new(code, m), h, c)
}

fn type_str(e: &mut LineEditor, s: &str) {
    let h = History::new();
    for c in s.chars() {
        key(e, &h, &NoCompleter, KeyCode::Char(c), Mods::NONE);
    }
}

fn press(e: &mut LineEditor, code: KeyCode) -> LineEvent {
    key(e, &History::new(), &NoCompleter, code, Mods::NONE)
}

fn ctrl(e: &mut LineEditor, c: char) -> LineEvent {
    key(
        e,
        &History::new(),
        &NoCompleter,
        KeyCode::Char(c),
        Mods::CTRL,
    )
}

#[test]
fn typing_and_submit() {
    let mut e = LineEditor::new("$ ");
    type_str(&mut e, "ls -l");
    assert_eq!(e.text(), "ls -l");
    assert_eq!(e.cursor(), 5);
    assert_eq!(
        press(&mut e, KeyCode::Enter),
        LineEvent::Submit("ls -l".into())
    );
    assert!(e.is_empty());
}

#[test]
fn insert_in_the_middle() {
    let mut e = LineEditor::new("");
    type_str(&mut e, "ac");
    press(&mut e, KeyCode::Left);
    type_str(&mut e, "b");
    assert_eq!(e.text(), "abc");
    assert_eq!(e.cursor(), 2);
}

#[test]
fn backspace_and_delete() {
    let mut e = LineEditor::new("");
    type_str(&mut e, "abcd");
    press(&mut e, KeyCode::Backspace);
    assert_eq!(e.text(), "abc");
    press(&mut e, KeyCode::Home);
    press(&mut e, KeyCode::Delete);
    assert_eq!(e.text(), "bc");
    assert_eq!(press(&mut e, KeyCode::Backspace), LineEvent::None);
    press(&mut e, KeyCode::End);
    assert_eq!(press(&mut e, KeyCode::Delete), LineEvent::None);
}

#[test]
fn home_end_and_ctrl_a_e() {
    let mut e = LineEditor::new("");
    type_str(&mut e, "hello");
    ctrl(&mut e, 'a');
    assert_eq!(e.cursor(), 0);
    ctrl(&mut e, 'e');
    assert_eq!(e.cursor(), 5);
    press(&mut e, KeyCode::Home);
    assert_eq!(e.cursor(), 0);
    press(&mut e, KeyCode::End);
    assert_eq!(e.cursor(), 5);
}

#[test]
fn ctrl_b_f_move_by_one() {
    let mut e = LineEditor::new("");
    type_str(&mut e, "ab");
    ctrl(&mut e, 'b');
    assert_eq!(e.cursor(), 1);
    ctrl(&mut e, 'f');
    ctrl(&mut e, 'f');
    assert_eq!(e.cursor(), 2);
}

#[test]
fn kill_and_yank() {
    let mut e = LineEditor::new("");
    type_str(&mut e, "one two three");
    ctrl(&mut e, 'w');
    assert_eq!(e.text(), "one two ");
    ctrl(&mut e, 'u');
    assert_eq!(e.text(), "");
    ctrl(&mut e, 'y');
    assert_eq!(e.text(), "one two ");
    press(&mut e, KeyCode::Home);
    for _ in 0..3 {
        press(&mut e, KeyCode::Right);
    }
    ctrl(&mut e, 'k');
    assert_eq!(e.text(), "one");
    ctrl(&mut e, 'y');
    assert_eq!(e.text(), "one two ");
}

#[test]
fn word_movement_with_ctrl_arrows() {
    let mut e = LineEditor::new("");
    type_str(&mut e, "foo bar baz");
    key(
        &mut e,
        &History::new(),
        &NoCompleter,
        KeyCode::Left,
        Mods::CTRL,
    );
    assert_eq!(e.cursor(), 8);
    key(
        &mut e,
        &History::new(),
        &NoCompleter,
        KeyCode::Left,
        Mods::CTRL,
    );
    assert_eq!(e.cursor(), 4);
    key(
        &mut e,
        &History::new(),
        &NoCompleter,
        KeyCode::Right,
        Mods::CTRL,
    );
    assert_eq!(e.cursor(), 7);
}

#[test]
fn ctrl_d_semantics() {
    let mut e = LineEditor::new("");
    assert_eq!(ctrl(&mut e, 'd'), LineEvent::Eof);
    type_str(&mut e, "ab");
    press(&mut e, KeyCode::Home);
    assert_eq!(ctrl(&mut e, 'd'), LineEvent::Changed);
    assert_eq!(e.text(), "b");
}

#[test]
fn ctrl_c_and_ctrl_l() {
    let mut e = LineEditor::new("");
    type_str(&mut e, "abc");
    assert_eq!(ctrl(&mut e, 'l'), LineEvent::ClearScreen);
    assert_eq!(e.text(), "abc");
    assert_eq!(ctrl(&mut e, 'c'), LineEvent::Interrupt);
    assert!(e.is_empty());
}

#[test]
fn unicode_editing() {
    let mut e = LineEditor::new("");
    type_str(&mut e, "ação€");
    assert_eq!(e.cursor(), 5);
    press(&mut e, KeyCode::Backspace);
    press(&mut e, KeyCode::Backspace);
    assert_eq!(e.text(), "açã");
}

#[test]
fn control_characters_are_not_inserted() {
    let mut e = LineEditor::new("");
    type_str(&mut e, "a\u{7}b\n");
    assert_eq!(e.text(), "ab");
}

#[test]
fn line_length_is_capped() {
    let mut e = LineEditor::new("");
    for _ in 0..MAX_LINE + 50 {
        e.insert('x');
    }
    assert_eq!(e.text().len(), MAX_LINE);
}

#[test]
fn display_shows_prompt_and_cursor_column() {
    let mut e = LineEditor::new("user@kitsune:/$ ");
    type_str(&mut e, "ls");
    let (t, col) = e.display();
    assert_eq!(t, "user@kitsune:/$ ls");
    assert_eq!(col, 18);
    press(&mut e, KeyCode::Left);
    assert_eq!(e.display().1, 17);
    e.set_prompt("> ");
    assert_eq!(e.display().0, "> ls");
}

fn hist(items: &[&str]) -> History {
    let mut h = History::new();
    for i in items {
        h.add(i);
    }
    h
}

#[test]
fn history_up_down_with_draft() {
    let h = hist(&["first", "second", "third"]);
    let mut e = LineEditor::new("");
    type_str(&mut e, "dra");
    key(&mut e, &h, &NoCompleter, KeyCode::Up, Mods::NONE);
    assert_eq!(e.text(), "third");
    key(&mut e, &h, &NoCompleter, KeyCode::Up, Mods::NONE);
    assert_eq!(e.text(), "second");
    key(&mut e, &h, &NoCompleter, KeyCode::Up, Mods::NONE);
    key(&mut e, &h, &NoCompleter, KeyCode::Up, Mods::NONE);
    assert_eq!(e.text(), "first");
    key(&mut e, &h, &NoCompleter, KeyCode::Down, Mods::NONE);
    assert_eq!(e.text(), "second");
    key(&mut e, &h, &NoCompleter, KeyCode::Down, Mods::NONE);
    key(&mut e, &h, &NoCompleter, KeyCode::Down, Mods::NONE);
    assert_eq!(e.text(), "dra");
    assert_eq!(
        key(&mut e, &h, &NoCompleter, KeyCode::Down, Mods::NONE),
        LineEvent::None
    );
}

#[test]
fn history_with_empty_history_does_nothing() {
    let mut e = LineEditor::new("");
    assert_eq!(press(&mut e, KeyCode::Up), LineEvent::None);
}

#[test]
fn history_entry_can_be_edited_and_submitted() {
    let h = hist(&["echo a"]);
    let mut e = LineEditor::new("");
    key(&mut e, &h, &NoCompleter, KeyCode::Up, Mods::NONE);
    type_str(&mut e, "b");
    assert_eq!(
        key(&mut e, &h, &NoCompleter, KeyCode::Enter, Mods::NONE),
        LineEvent::Submit("echo ab".into())
    );
}

#[test]
fn ctrl_p_n_navigate_history() {
    let h = hist(&["x", "y"]);
    let mut e = LineEditor::new("");
    key(&mut e, &h, &NoCompleter, KeyCode::Char('p'), Mods::CTRL);
    assert_eq!(e.text(), "y");
    key(&mut e, &h, &NoCompleter, KeyCode::Char('n'), Mods::CTRL);
    assert_eq!(e.text(), "");
}

#[test]
fn reverse_search_finds_and_cycles() {
    let _lang = crate::i18n::testlang::LangGuard::new(crate::i18n::Lang::En);
    let h = hist(&["echo one", "ls", "echo two", "pwd"]);
    let mut e = LineEditor::new("$ ");
    let k = |e: &mut LineEditor, c: KeyCode, m: Mods| key(e, &h, &NoCompleter, c, m);
    k(&mut e, KeyCode::Char('r'), Mods::CTRL);
    assert!(e.is_searching());
    for c in "echo".chars() {
        k(&mut e, KeyCode::Char(c), Mods::NONE);
    }
    assert_eq!(e.text(), "echo two");
    assert!(
        e.display()
            .0
            .starts_with("(reverse-i-search)`echo': echo two")
    );
    // The search prompt follows the language, and the column counts its characters.
    let _pt = crate::i18n::testlang::LangGuard::new(crate::i18n::Lang::Pt);
    let (text, col) = e.display();
    assert!(
        text.starts_with("(busca reversa)`echo': echo two"),
        "{text}"
    );
    assert_eq!(col, "(busca reversa)`echo': ".chars().count());
    k(&mut e, KeyCode::Char('r'), Mods::CTRL);
    assert_eq!(e.text(), "echo one");
    k(&mut e, KeyCode::Char('r'), Mods::CTRL);
    assert_eq!(e.text(), "echo one");
    assert_eq!(
        k(&mut e, KeyCode::Enter, Mods::NONE),
        LineEvent::Submit("echo one".into())
    );
    assert!(!e.is_searching());
}

#[test]
fn reverse_search_cancel_restores_the_line() {
    let h = hist(&["alpha", "beta"]);
    let mut e = LineEditor::new("");
    type_str(&mut e, "draft");
    let k = |e: &mut LineEditor, c: KeyCode, m: Mods| key(e, &h, &NoCompleter, c, m);
    k(&mut e, KeyCode::Char('r'), Mods::CTRL);
    k(&mut e, KeyCode::Char('a'), Mods::NONE);
    assert_eq!(e.text(), "beta");
    k(&mut e, KeyCode::Esc, Mods::NONE);
    assert_eq!(e.text(), "draft");
    assert!(!e.is_searching());
    k(&mut e, KeyCode::Char('r'), Mods::CTRL);
    k(&mut e, KeyCode::Char('g'), Mods::CTRL);
    assert_eq!(e.text(), "draft");
}

#[test]
fn reverse_search_accept_with_arrow_keeps_editing() {
    let h = hist(&["make all"]);
    let mut e = LineEditor::new("");
    let k = |e: &mut LineEditor, c: KeyCode, m: Mods| key(e, &h, &NoCompleter, c, m);
    k(&mut e, KeyCode::Char('r'), Mods::CTRL);
    k(&mut e, KeyCode::Char('m'), Mods::NONE);
    k(&mut e, KeyCode::Left, Mods::NONE);
    assert!(!e.is_searching());
    assert_eq!(e.text(), "make all");
    k(&mut e, KeyCode::Char('!'), Mods::NONE);
    assert_eq!(e.text(), "make all!");
}

#[test]
fn reverse_search_backspace_and_no_match() {
    let h = hist(&["foo", "bar"]);
    let mut e = LineEditor::new("");
    let k = |e: &mut LineEditor, c: KeyCode, m: Mods| key(e, &h, &NoCompleter, c, m);
    k(&mut e, KeyCode::Char('r'), Mods::CTRL);
    k(&mut e, KeyCode::Char('f'), Mods::NONE);
    assert_eq!(e.text(), "foo");
    k(&mut e, KeyCode::Char('z'), Mods::NONE);
    k(&mut e, KeyCode::Backspace, Mods::NONE);
    assert_eq!(e.text(), "foo");
    k(&mut e, KeyCode::Backspace, Mods::NONE);
    assert_eq!(e.text(), "");
    k(&mut e, KeyCode::Char('q'), Mods::NONE);
    assert_eq!(k(&mut e, KeyCode::Enter, Mods::NONE), LineEvent::Changed);
    assert!(!e.is_searching());
}

#[test]
fn ctrl_c_in_search_interrupts() {
    let h = hist(&["a"]);
    let mut e = LineEditor::new("");
    let k = |e: &mut LineEditor, c: KeyCode, m: Mods| key(e, &h, &NoCompleter, c, m);
    k(&mut e, KeyCode::Char('r'), Mods::CTRL);
    assert_eq!(
        k(&mut e, KeyCode::Char('c'), Mods::CTRL),
        LineEvent::Interrupt
    );
    assert!(!e.is_searching());
}

// ---- completion -------------------------------------------------------------

struct World {
    sh: Shell,
    fs: MemFs,
    sys: MockSys,
    ed: LineEditor,
}

impl World {
    fn new() -> Self {
        let fs = MemFs::new()
            .with_file("/notes.txt", b"")
            .with_file("/note2.txt", b"")
            .with_file("/.hidden", b"")
            .with_dir("/docs")
            .with_dir("/docs/sub")
            .with_file("/docs/readme.md", b"")
            .with_file("/docs/my file.txt", b"")
            .with_dir("/bin")
            .with_file("/bin/hello.sh", b"echo hi\n");
        Self {
            sh: Shell::new(),
            fs,
            sys: MockSys::default(),
            ed: LineEditor::new("$ "),
        }
    }

    fn tab(&mut self, typed: &str) -> LineEvent {
        self.ed.set_text(typed);
        let comp = ShellCompleter {
            shell: &self.sh,
            fs: &self.fs,
        };
        self.ed
            .handle_key(KeyEvent::plain(KeyCode::Tab), &History::new(), &comp)
    }
}

#[test]
fn complete_unique_command() {
    let mut w = World::new();
    assert_eq!(w.tab("whi"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "which ");
    assert_eq!(w.tab("unal"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "unalias ");
}

#[test]
fn complete_command_common_prefix_then_list() {
    let mut w = World::new();
    // "hi" matches only `history`; "he" matches help/head.
    assert_eq!(w.tab("his"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "history ");
    match w.tab("he") {
        LineEvent::Candidates(c) => assert_eq!(c, ["head", "hello", "help"]),
        e => panic!("{e:?}"),
    }
    assert_eq!(w.ed.text(), "he");
    assert_eq!(w.tab("cl"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "clear ");
}

#[test]
fn complete_extends_to_common_prefix() {
    let mut w = World::new();
    assert_eq!(w.tab("mk"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "mkdir ");
    assert_eq!(w.tab("tou"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "touch ");
    // "ec" is unique too; use a real shared prefix: "un" -> uniq/unset/unalias share only "un".
    match w.tab("un") {
        LineEvent::Candidates(c) => assert_eq!(c, ["unalias", "uniq", "unset", "unzip"]),
        e => panic!("{e:?}"),
    }
}

#[test]
fn complete_script_from_path_without_extension() {
    let mut w = World::new();
    assert_eq!(w.tab("hell"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "hello ");
}

#[test]
fn complete_aliases_and_functions() {
    let mut w = World::new();
    {
        let mut h = Host {
            fs: &mut w.fs,
            sys: &mut w.sys,
        };
        w.sh.run_line("alias zzalias='echo a'", &mut h);
        w.sh.run_line("zzfunc() { :; }", &mut h);
    }
    assert_eq!(w.tab("zza"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "zzalias ");
    assert_eq!(w.tab("zzf"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "zzfunc ");
}

#[test]
fn complete_paths_files_and_dirs() {
    let mut w = World::new();
    assert_eq!(w.tab("cat not"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "cat note");
    match w.tab("cat note") {
        LineEvent::Candidates(c) => assert_eq!(c, ["note2.txt", "notes.txt"]),
        e => panic!("{e:?}"),
    }
    assert_eq!(w.tab("cd do"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "cd docs/");
    assert_eq!(w.tab("cat docs/re"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "cat docs/readme.md ");
    assert_eq!(w.tab("ls /do"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "ls /docs/");
}

#[test]
fn complete_hides_dotfiles_unless_asked() {
    let mut w = World::new();
    assert_eq!(w.tab("cat ."), LineEvent::Changed);
    assert_eq!(w.ed.text(), "cat .hidden ");
    match w.tab("cat ") {
        LineEvent::Candidates(c) => assert!(!c.iter().any(|n| n.starts_with('.'))),
        e => panic!("{e:?}"),
    }
}

#[test]
fn complete_escapes_spaces() {
    let mut w = World::new();
    assert_eq!(w.tab("cat docs/my"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "cat docs/my\\ file.txt ");
}

#[test]
fn complete_inside_quotes() {
    let mut w = World::new();
    assert_eq!(w.tab("cat \"docs/my"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "cat \"docs/my file.txt\" ");
}

#[test]
fn complete_command_position_after_operators() {
    let mut w = World::new();
    assert_eq!(w.tab("echo hi | whi"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "echo hi | which ");
    assert_eq!(w.tab("true && unal"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "true && unalias ");
    assert_eq!(w.tab("echo a; whi"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "echo a; which ");
    assert_eq!(w.tab("if whi"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "if which ");
}

#[test]
fn complete_argument_position_uses_paths_not_commands() {
    let mut w = World::new();
    assert_eq!(w.tab("echo whi"), LineEvent::None);
    assert_eq!(w.ed.text(), "echo whi");
}

#[test]
fn complete_variables() {
    let mut w = World::new();
    assert_eq!(w.tab("echo $HO"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "echo $HOME ");
    assert_eq!(w.tab("echo $PA"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "echo $PATH ");
}

#[test]
fn complete_in_the_middle_of_a_line_keeps_the_tail() {
    let mut w = World::new();
    w.ed.set_text("cat not rest");
    for _ in 0..5 {
        w.ed.handle_key(
            KeyEvent::plain(KeyCode::Left),
            &History::new(),
            &NoCompleter,
        );
    }
    let comp = ShellCompleter {
        shell: &w.sh,
        fs: &w.fs,
    };
    w.ed.handle_key(KeyEvent::plain(KeyCode::Tab), &History::new(), &comp);
    assert_eq!(w.ed.text(), "cat note rest");
    assert_eq!(w.ed.cursor(), "cat note".len());
}

#[test]
fn complete_nothing_is_a_noop() {
    let mut w = World::new();
    assert_eq!(w.tab("cat zzz"), LineEvent::None);
    assert_eq!(w.ed.text(), "cat zzz");
    assert_eq!(w.tab("zzzz"), LineEvent::None);
}

#[test]
fn complete_relative_to_cwd() {
    let mut w = World::new();
    w.fs.set_cwd("/docs").unwrap();
    assert_eq!(w.tab("cat re"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "cat readme.md ");
    assert_eq!(w.tab("cd ../no"), LineEvent::Changed);
    assert_eq!(w.ed.text(), "cd ../note");
}

#[test]
fn full_session_through_the_shell() {
    let mut w = World::new();
    let mut submitted = None;
    let typed = "echo hi";
    for c in typed.chars() {
        w.ed.handle_key(KeyEvent::ch(c), w.sh.history(), &NoCompleter);
    }
    if let LineEvent::Submit(line) = w.ed.handle_key(
        KeyEvent::plain(KeyCode::Enter),
        w.sh.history(),
        &NoCompleter,
    ) {
        let mut h = Host {
            fs: &mut w.fs,
            sys: &mut w.sys,
        };
        submitted = Some(w.sh.run_line(&line, &mut h));
    }
    assert_eq!(submitted.unwrap().text(), "hi\n");
    // History now has the line: Up recalls it.
    w.ed.handle_key(KeyEvent::plain(KeyCode::Up), w.sh.history(), &NoCompleter);
    assert_eq!(w.ed.text(), "echo hi");
}

#[test]
fn common_prefix_helper() {
    assert_eq!(common_prefix(&["abc", "abd", "ab"]), "ab");
    assert_eq!(common_prefix(&["x"]), "x");
    assert_eq!(common_prefix(&[]), "");
    assert_eq!(common_prefix(&["é1", "é2"]), "é");
}

#[test]
fn random_keys_never_panic() {
    let codes = [
        KeyCode::Char('a'),
        KeyCode::Char(' '),
        KeyCode::Char('\''),
        KeyCode::Char('"'),
        KeyCode::Char('$'),
        KeyCode::Char('/'),
        KeyCode::Char('|'),
        KeyCode::Char('r'),
        KeyCode::Char('g'),
        KeyCode::Enter,
        KeyCode::Backspace,
        KeyCode::Delete,
        KeyCode::Tab,
        KeyCode::Esc,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::PageUp,
        KeyCode::F(3),
    ];
    let mut w = World::new();
    let mut hist = History::new();
    for c in ["echo a", "ls /docs", "cat notes.txt"] {
        hist.add(c);
    }
    let mut x: u64 = 31337;
    for _ in 0..5000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let code = codes[(x % codes.len() as u64) as usize];
        let m = Mods {
            ctrl: (x >> 8) & 3 == 0,
            shift: (x >> 10) & 3 == 0,
            alt: (x >> 12) & 7 == 0,
        };
        let comp = ShellCompleter {
            shell: &w.sh,
            fs: &w.fs,
        };
        let _ = w.ed.handle_key(KeyEvent::new(code, m), &hist, &comp);
        assert!(w.ed.cursor() <= w.ed.text().chars().count());
        let (_, col) = w.ed.display();
        let _ = col;
    }
}
