//! Dead-key composition: an accent key followed by a letter gives the accented letter.

/// US-International style accents for typing Portuguese on a US keyboard:
/// `'` `` ` `` `~` `^` `"` are *dead keys*. Typed before a vowel (or `c`, `n`,
/// `y`) they make the accented letter (`'` then `a` is `\u{e1}`, `'` then `c`
/// is `\u{e7}`); before a space or the same key they give the plain
/// character; before anything else both come out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Compose {
    pub(crate) pending: Option<char>,
}

/// What a keystroke produced: up to two characters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Composed {
    pub(crate) chars: [Option<char>; 2],
}

impl Composed {
    pub(crate) fn none() -> Self {
        Composed { chars: [None; 2] }
    }
    pub(crate) fn one(a: char) -> Self {
        Composed {
            chars: [Some(a), None],
        }
    }
    pub(crate) fn two(a: char, b: char) -> Self {
        Composed {
            chars: [Some(a), Some(b)],
        }
    }
    /// The produced characters, in order.
    pub fn iter(&self) -> impl Iterator<Item = char> + '_ {
        self.chars.iter().flatten().copied()
    }
}

pub(crate) fn is_dead(c: char) -> bool {
    matches!(c, '\'' | '`' | '~' | '^' | '"')
}

/// The accented letter for dead key `d` followed by `c`, if there is one.
pub fn accent(d: char, c: char) -> Option<char> {
    let (lower, upper): (&str, &str) = match d {
        '\'' => (
            "a\u{e1}e\u{e9}i\u{ed}o\u{f3}u\u{fa}c\u{e7}y\u{fd}",
            "A\u{c1}E\u{c9}I\u{cd}O\u{d3}U\u{da}C\u{c7}Y\u{dd}",
        ),
        '`' => (
            "a\u{e0}e\u{e8}i\u{ec}o\u{f2}u\u{f9}",
            "A\u{c0}E\u{c8}I\u{cc}O\u{d2}U\u{d9}",
        ),
        '~' => ("a\u{e3}o\u{f5}n\u{f1}", "A\u{c3}O\u{d5}N\u{d1}"),
        '^' => (
            "a\u{e2}e\u{ea}i\u{ee}o\u{f4}u\u{fb}",
            "A\u{c2}E\u{ca}I\u{ce}O\u{d4}U\u{db}",
        ),
        '"' => (
            "a\u{e4}e\u{eb}i\u{ef}o\u{f6}u\u{fc}y\u{ff}",
            "A\u{c4}E\u{cb}I\u{cf}O\u{d6}U\u{dc}",
        ),
        _ => return None,
    };
    for table in [lower, upper] {
        let mut it = table.chars();
        while let (Some(base), Some(acc)) = (it.next(), it.next()) {
            if base == c {
                return Some(acc);
            }
        }
    }
    None
}

impl Compose {
    pub const fn new() -> Self {
        Compose { pending: None }
    }

    /// True while a dead key is waiting for the next character.
    pub fn pending(&self) -> Option<char> {
        self.pending
    }

    /// Feed one typed character.
    pub fn feed(&mut self, c: char) -> Composed {
        match self.pending.take() {
            None if is_dead(c) => {
                self.pending = Some(c);
                Composed::none()
            }
            None => Composed::one(c),
            Some(d) => {
                if c == ' ' || c == d {
                    Composed::one(d)
                } else if let Some(a) = accent(d, c) {
                    Composed::one(a)
                } else if is_dead(c) {
                    self.pending = Some(c);
                    Composed::one(d)
                } else {
                    Composed::two(d, c)
                }
            }
        }
    }

    /// A non-character key arrived: a waiting dead key comes out as itself.
    pub fn flush(&mut self) -> Composed {
        match self.pending.take() {
            Some(d) => Composed::one(d),
            None => Composed::none(),
        }
    }
}

// ---- editing state ----
