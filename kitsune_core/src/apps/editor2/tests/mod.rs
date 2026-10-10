//! Behaviour tests for the v2 editor, including a randomized comparison
//! against a trivial `Vec<String>` model.

use super::*;
use crate::system::input::{KeyCode, KeyEvent, Mods};
use alloc::string::ToString;
use alloc::vec;

fn ed(s: &str) -> Editor {
    Editor::from_bytes(s.as_bytes())
}

fn text(e: &Editor) -> String {
    String::from_utf8_lossy(&e.to_bytes()).into_owned()
}

fn type_str(e: &mut Editor, s: &str) {
    for c in s.chars() {
        e.insert_char(c);
    }
}

fn press(e: &mut Editor, clip: &mut Clipboard, code: KeyCode, mods: Mods) -> Event {
    e.handle_key(KeyEvent::new(code, mods), clip)
}

fn key(e: &mut Editor, code: KeyCode) {
    let mut c = Clipboard::new();
    press(e, &mut c, code, Mods::NONE);
}

fn skey(e: &mut Editor, code: KeyCode) {
    let mut c = Clipboard::new();
    press(e, &mut c, code, Mods::SHIFT);
}

fn ckey(e: &mut Editor, code: KeyCode) {
    let mut c = Clipboard::new();
    press(e, &mut c, code, Mods::CTRL);
}

fn check(e: &Editor) {
    assert!(e.text.lines_consistent(), "line index out of sync");
    let c = e.cursor;
    assert!(c <= e.text.len());
    let l = e.text.line_of(c);
    assert!(c <= e.text.line_end(l), "cursor past line content");
    assert!(c >= e.text.line_start(l));
    assert_eq!(e.normalize(c), c, "cursor not normalized");
    if let Some(a) = e.anchor {
        assert!(a <= e.text.len());
    }
}

fn numbered(n: usize) -> String {
    (0..n).map(|i| format!("line {i}\n")).collect()
}

fn big_doc(bytes: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(bytes + 100);
    let mut i = 0usize;
    while v.len() < bytes {
        v.extend_from_slice(
            format!("line {i}: the quick brown fox jumps over ñandú €\n").as_bytes(),
        );
        i += 1;
    }
    v
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[derive(Clone)]
struct Model {
    lines: Vec<String>,
    cur: (usize, usize),
    anchor: Option<(usize, usize)>,
    pref: usize,
}

fn cl(s: &str) -> usize {
    s.chars().count()
}

fn bi(s: &str, c: usize) -> usize {
    s.char_indices().nth(c).map_or(s.len(), |x| x.0)
}

impl Model {
    fn new(init: &str) -> Self {
        Self {
            lines: init.split('\n').map(String::from).collect(),
            cur: (0, 0),
            anchor: None,
            pref: 0,
        }
    }

    fn text(&self) -> String {
        self.lines.join("\n")
    }

    fn offset(&self, p: (usize, usize)) -> usize {
        let n: usize = self.lines.iter().take(p.0).map(|s| s.len() + 1).sum();
        n + bi(&self.lines[p.0], p.1)
    }

    fn sel(&self) -> Option<((usize, usize), (usize, usize))> {
        let a = self.anchor?;
        if a == self.cur {
            None
        } else {
            Some((a.min(self.cur), a.max(self.cur)))
        }
    }

    fn delete_range(&mut self, a: (usize, usize), b: (usize, usize)) {
        if a.0 == b.0 {
            let s = &mut self.lines[a.0];
            let (i, j) = (bi(s, a.1), bi(s, b.1));
            s.replace_range(i..j, "");
        } else {
            let tail = self.lines[b.0][bi(&self.lines[b.0], b.1)..].to_string();
            let h = bi(&self.lines[a.0], a.1);
            self.lines[a.0].truncate(h);
            self.lines[a.0].push_str(&tail);
            self.lines.drain(a.0 + 1..=b.0);
        }
        self.cur = a;
        self.anchor = None;
    }

    fn del_sel(&mut self) -> bool {
        if let Some((a, b)) = self.sel() {
            self.delete_range(a, b);
            true
        } else {
            false
        }
    }

    fn type_char(&mut self, c: char) {
        self.del_sel();
        let (l, col) = self.cur;
        let i = bi(&self.lines[l], col);
        self.lines[l].insert(i, c);
        self.cur.1 += 1;
        self.anchor = None;
        self.pref = self.cur.1;
    }

    fn enter(&mut self) {
        self.del_sel();
        let (l, col) = self.cur;
        let i = bi(&self.lines[l], col);
        let tail = self.lines[l].split_off(i);
        self.lines.insert(l + 1, tail);
        self.cur = (l + 1, 0);
        self.anchor = None;
        self.pref = 0;
    }

    fn backspace(&mut self) {
        if self.del_sel() {
            self.pref = self.cur.1;
            return;
        }
        let (l, c) = self.cur;
        if c > 0 {
            self.delete_range((l, c - 1), (l, c));
        } else if l > 0 {
            self.delete_range((l - 1, cl(&self.lines[l - 1])), (l, 0));
        } else {
            // No-op at the start of the document: the column is untouched.
            return;
        }
        self.pref = self.cur.1;
    }

    fn delete(&mut self) {
        if self.del_sel() {
            self.pref = self.cur.1;
            return;
        }
        let (l, c) = self.cur;
        if c < cl(&self.lines[l]) {
            self.delete_range((l, c), (l, c + 1));
            self.cur = (l, c);
        } else if l + 1 < self.lines.len() {
            self.delete_range((l, c), (l + 1, 0));
        } else {
            return;
        }
        self.pref = self.cur.1;
    }

    fn prep(&mut self, shift: bool) {
        if shift {
            if self.anchor.is_none() {
                self.anchor = Some(self.cur);
            }
        } else {
            self.anchor = None;
        }
    }

    fn left(&mut self, shift: bool) {
        if !shift && let Some((a, _)) = self.sel() {
            self.cur = a;
            self.anchor = None;
            self.pref = a.1;
            return;
        }
        self.prep(shift);
        let (l, c) = self.cur;
        self.cur = if c > 0 {
            (l, c - 1)
        } else if l > 0 {
            (l - 1, cl(&self.lines[l - 1]))
        } else {
            (0, 0)
        };
        self.pref = self.cur.1;
    }

    fn right(&mut self, shift: bool) {
        if !shift && let Some((_, b)) = self.sel() {
            self.cur = b;
            self.anchor = None;
            self.pref = b.1;
            return;
        }
        self.prep(shift);
        let (l, c) = self.cur;
        self.cur = if c < cl(&self.lines[l]) {
            (l, c + 1)
        } else if l + 1 < self.lines.len() {
            (l + 1, 0)
        } else {
            (l, c)
        };
        self.pref = self.cur.1;
    }

    fn up(&mut self, shift: bool) {
        self.prep(shift);
        let (l, _) = self.cur;
        self.cur = if l == 0 {
            (0, 0)
        } else {
            (l - 1, self.pref.min(cl(&self.lines[l - 1])))
        };
    }

    fn down(&mut self, shift: bool) {
        self.prep(shift);
        let (l, _) = self.cur;
        let last = self.lines.len() - 1;
        self.cur = if l == last {
            (last, cl(&self.lines[last]))
        } else {
            (l + 1, self.pref.min(cl(&self.lines[l + 1])))
        };
    }

    fn end(&mut self, shift: bool) {
        self.prep(shift);
        self.cur.1 = cl(&self.lines[self.cur.0]);
        self.pref = self.cur.1;
    }

    fn select_all(&mut self) {
        self.anchor = Some((0, 0));
        let l = self.lines.len() - 1;
        self.cur = (l, cl(&self.lines[l]));
        self.pref = self.cur.1;
    }
}

const ALPHABET: [char; 10] = ['a', 'b', ' ', 'z', 'é', '€', '😀', 'x', '.', 'Q'];

fn random_session(seed: u64, steps: usize, init: &str) {
    let mut rng = Rng(seed);
    let mut e = ed(init);
    e.set_auto_indent(false);
    e.resize(1 + rng.below(12), 1 + rng.below(30));
    let mut m = Model::new(init);
    for step in 0..steps {
        let last_op = rng.below(16);
        match last_op {
            0..=4 => {
                let c = ALPHABET[rng.below(ALPHABET.len())];
                e.insert_char(c);
                m.type_char(c);
            }
            5 => {
                e.newline();
                m.enter();
            }
            6 => {
                e.backspace();
                m.backspace();
            }
            7 => {
                e.delete_forward();
                m.delete();
            }
            8 => {
                let s = rng.below(2) == 1;
                e.move_left(s);
                m.left(s);
            }
            9 => {
                let s = rng.below(2) == 1;
                e.move_right(s);
                m.right(s);
            }
            10 => {
                let s = rng.below(2) == 1;
                e.move_vertical(-1, s);
                m.up(s);
            }
            11 => {
                let s = rng.below(2) == 1;
                e.move_vertical(1, s);
                m.down(s);
            }
            12 => {
                let s = rng.below(2) == 1;
                e.move_end(s);
                m.end(s);
            }
            13 => {
                if rng.below(4) == 0 {
                    e.select_all();
                    m.select_all();
                }
            }
            14 => {
                // Undo then redo is the identity.
                let before = e.to_bytes();
                if e.undo() {
                    assert!(e.redo());
                    assert_eq!(e.to_bytes(), before, "seed {seed} step {step}");
                    // Selection and pref column are not restored by undo.
                    m.anchor = None;
                    e.clear_selection();
                    m.cur = {
                        let (l, c) = e.cursor();
                        (l, c)
                    };
                    m.pref = m.cur.1;
                }
            }
            _ => {
                e.resize(1 + rng.below(12), 1 + rng.below(30));
            }
        }
        assert_eq!(
            String::from_utf8_lossy(&e.to_bytes()),
            m.text(),
            "text diverged: seed {seed} step {step}"
        );
        assert_eq!(
            e.cursor(),
            m.cur,
            "cursor diverged: seed {seed} step {step} op {last_op}"
        );
        let want = m.sel().map(|(a, b)| (m.offset(a), m.offset(b)));
        assert_eq!(e.selection(), want, "selection: seed {seed} step {step}");
        if step % 25 == 0 {
            check(&e);
        }
    }
    check(&e);
    // Undo everything: back to the original; redo everything: back to the end.
    let end_text = e.to_bytes();
    while e.undo() {}
    assert_eq!(e.to_bytes(), init.as_bytes(), "undo-all seed {seed}");
    assert!(!e.is_modified());
    while e.redo() {}
    assert_eq!(e.to_bytes(), end_text, "redo-all seed {seed}");
    check(&e);
}

mod basic_editing;
mod clipboard;
mod encodings;
mod keys;
mod large_inputs;
mod line_endings;
mod movement;
mod randomized_model_comparison;
mod search_replace;
mod selection;
mod tab_indent;
mod undo_redo;
mod utf_8;
mod viewport;
mod word_deletion;
