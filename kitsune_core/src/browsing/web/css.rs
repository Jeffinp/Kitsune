//! CSS parser: rules, selectors (type, class, id, attribute, descendant and
//! child combinators), specificity and declaration lists.
//!
//! Anything the engine cannot honour is dropped at parse time instead of being
//! approximated: a selector with a pseudo-class (`a:hover`), a sibling combinator
//! or a pseudo-element never matches, so it cannot restyle the wrong elements.

use super::dom::Element;

mod media;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
pub use media::{DEFAULT_H, DEFAULT_W, Media, set_viewport, viewport_h, viewport_w};

// ---- CSS ----

/// Maximum number of rules kept from one stylesheet.
///
/// The cascade tests every rule against every element, so rules x elements is
/// the cost. With `MAX_NODES` elements this bounds the whole cascade to a few
/// million selector tests. Later rules are dropped (document order is cascade
/// order, so the page keeps its earliest, usually most structural, rules).
pub const MAX_RULES: usize = 20_000;

/// Maximum selectors summed over all rules of one stylesheet: a single rule
/// with a huge comma list would otherwise dodge [`MAX_RULES`].
pub const MAX_SELECTORS: usize = 40_000;

/// Longest compound chain of a selector (`a b > c d` is 4).
pub const MAX_COMPOUNDS: usize = 5;

/// Longest selector text considered, in bytes.
const MAX_SELECTOR_LEN: usize = 256;

/// Longest stylesheet text parsed, in bytes (the HTML body is capped far below this).
const MAX_CSS_BYTES: usize = 768 * 1024;

/// A parsed stylesheet: an ordered list of rules.
#[derive(Debug, Default)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
    /// The conditions of the `@media` blocks, referred to by [`Rule::media`].
    pub medias: Vec<Media>,
    /// Addresses of the `@import`ed sheets, in order.
    pub imports: Vec<String>,
    /// The rules that can match an element, found by what it is called, its `id` and its classes: per
    /// name, the indices (in rule order) of the rules whose selectors end in that tag, id or class.
    /// Empty until [`build_index`].
    by_tag: alloc::collections::BTreeMap<String, Vec<u32>>,
    by_class: alloc::collections::BTreeMap<String, Vec<u32>>,
    by_id: alloc::collections::BTreeMap<String, Vec<u32>>,
    /// The rules that name nothing to index by (`*`, `[attr]`, `:root`...).
    generic: Vec<u32>,
    indexed: bool,
}

impl Stylesheet {
    /// Index the rules by the tag, id or class their selectors end in, so the cascade tests an
    /// element against the few rules that can match it instead of every rule. Call after the
    /// rule list is final.
    pub fn build_index(&mut self) {
        self.by_tag.clear();
        self.by_class.clear();
        self.by_id.clear();
        self.generic.clear();
        for (i, r) in self.rules.iter().enumerate() {
            let i = i as u32;
            // A rule goes under the best key of each of its selectors; one without any key makes
            // the whole rule generic.
            let mut keyed = true;
            for s in &r.selectors {
                let map = if let Some(id) = &s.id {
                    Some((&mut self.by_id, id.clone()))
                } else if let Some(c) = s.classes.first() {
                    Some((&mut self.by_class, c.clone()))
                } else if let Some(t) = s.tag.as_ref().filter(|t| *t != "*") {
                    Some((&mut self.by_tag, t.clone()))
                } else {
                    None
                };
                match map {
                    Some((m, k)) => {
                        let v = m.entry(k).or_default();
                        if v.last() != Some(&i) {
                            v.push(i);
                        }
                    }
                    None => keyed = false,
                }
            }
            if !keyed {
                self.generic.push(i);
            }
        }
        self.indexed = true;
    }

    /// Does `rule` apply on a screen as wide as the current viewport?
    pub fn rule_applies(&self, rule: &Rule) -> bool {
        rule.media == 0
            || self
                .medias
                .get(usize::from(rule.media) - 1)
                .is_some_and(|m| m.matches(viewport_w(), viewport_h()))
    }

    /// The rules worth testing for `el`, in rule order, written to `out` (replacing what it held);
    /// `false` when the sheet was not indexed (test them all).
    pub fn candidates(&self, el: &Element, out: &mut Vec<u32>) -> bool {
        if !self.indexed {
            return false;
        }
        out.clear();
        out.extend_from_slice(&self.generic);
        if let Some(v) = self.by_tag.get(el.tag.as_str()) {
            out.extend_from_slice(v);
        }
        if !self.by_class.is_empty() {
            for c in el.classes() {
                if let Some(v) = self.by_class.get(c) {
                    out.extend_from_slice(v);
                }
            }
        }
        if let Some(id) = el.id()
            && let Some(v) = self.by_id.get(id)
        {
            out.extend_from_slice(v);
        }
        out.sort_unstable();
        out.dedup();
        true
    }
}

#[derive(Debug)]
pub struct Rule {
    pub selectors: Vec<Selector>,
    pub decls: Vec<Decl>,
    /// From the page (true) or from the user-agent sheet (false). Presentational attributes
    /// beat the second kind and lose to the first.
    pub author: bool,
    /// `1 + index` into [`Stylesheet::medias`] when the rule sits in an `@media` block (0 = always).
    pub media: u16,
}

/// How an attribute selector compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttrOp {
    /// `[name]`
    Exists,
    /// `[name=value]`
    Equals,
    /// `[name~=value]` (one of the space separated words)
    Word,
    /// `[name^=value]`
    Prefix,
    /// `[name$=value]`
    Suffix,
    /// `[name*=value]`
    Contains,
}

#[derive(Debug, Clone)]
pub struct AttrSel {
    pub name: String,
    pub op: AttrOp,
    pub value: String,
}

impl AttrSel {
    fn matches(&self, el: &Element) -> bool {
        let Some(v) = el.attrs.get(self.name.as_str()) else {
            return false;
        };
        let (v, want) = (v.as_str(), self.value.as_str());
        let eq = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
        match self.op {
            AttrOp::Exists => true,
            AttrOp::Equals => eq(v, want),
            AttrOp::Word => v.split_ascii_whitespace().any(|w| eq(w, want)),
            AttrOp::Prefix => {
                !want.is_empty() && v.len() >= want.len() && eq(&v[..want.len()], want)
            }
            AttrOp::Suffix => {
                !want.is_empty()
                    && v.len() >= want.len()
                    && v.get(v.len() - want.len()..).is_some_and(|t| eq(t, want))
            }
            AttrOp::Contains => {
                !want.is_empty() && v.to_ascii_lowercase().contains(&want.to_ascii_lowercase())
            }
        }
    }
}

/// A pseudo-class the engine can answer from the element and its place among its siblings.
#[derive(Debug, Clone)]
pub enum Pseudo {
    /// `:nth-child(an+b)` (`first-child` is `0n+1`, `last-child` is `nth-last-child(1)`).
    Nth {
        a: i32,
        b: i32,
        last: bool,
        of_type: bool,
    },
    OnlyChild,
    Empty,
    /// `:link`: an anchor with an address.
    Link,
    /// `:checked`, `:disabled`, `:enabled`: from the attributes.
    Checked,
    Disabled,
    Enabled,
    /// `:not(...)`: none of the compounds match.
    Not(Vec<Compound>),
    /// `:is(...)` / `:where(...)`: one of the compounds matches.
    Any(Vec<Compound>),
}

impl Pseudo {
    fn matches(&self, el: &Element) -> bool {
        match self {
            Pseudo::Nth {
                a,
                b,
                last,
                of_type,
            } => {
                let (idx, cnt) = if *of_type {
                    (el.sib.tindex, el.sib.tcount)
                } else {
                    (el.sib.index, el.sib.count)
                };
                if idx == 0 {
                    return false;
                }
                let pos = if *last { cnt - idx + 1 } else { idx };
                nth_matches(*a, *b, i32::from(pos))
            }
            Pseudo::OnlyChild => el.sib.count == 1,
            Pseudo::Empty => el.children.is_empty(),
            Pseudo::Link => {
                matches!(el.tag.as_str(), "a" | "area") && el.attrs.contains_key("href")
            }
            Pseudo::Checked => {
                el.attrs.contains_key("checked") || el.attrs.contains_key("selected")
            }
            Pseudo::Disabled => el.attrs.contains_key("disabled"),
            Pseudo::Enabled => {
                matches!(el.tag.as_str(), "input" | "button" | "select" | "textarea")
                    && !el.attrs.contains_key("disabled")
            }
            Pseudo::Not(list) => !list.iter().any(|c| c.matches(el)),
            Pseudo::Any(list) => list.iter().any(|c| c.matches(el)),
        }
    }

    fn specificity(&self) -> (usize, usize, usize) {
        match self {
            Pseudo::Not(l) => l.iter().map(|c| c.specificity()).max().unwrap_or((0, 0, 0)),
            // `:where()` should count for nothing and `:is()` for its strongest argument; both are
            // rare enough that the second reading serves.
            Pseudo::Any(l) => l.iter().map(|c| c.specificity()).max().unwrap_or((0, 0, 0)),
            _ => (0, 1, 0),
        }
    }
}

/// Does position `pos` (1-based) satisfy `an + b` for some `n >= 0`?
fn nth_matches(a: i32, b: i32, pos: i32) -> bool {
    if a == 0 {
        return pos == b;
    }
    let d = pos - b;
    d % a == 0 && d / a >= 0
}

/// One compound selector: `tag#id.class[attr]:pseudo`.
#[derive(Debug, Default, Clone)]
pub struct Compound {
    pub tag: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub attrs: Vec<AttrSel>,
    pub pseudos: Vec<Pseudo>,
}

impl Compound {
    fn matches(&self, el: &Element) -> bool {
        if let Some(t) = &self.tag
            && t != "*"
            && *t != el.tag
        {
            return false;
        }
        if let Some(id) = &self.id
            && el.id() != Some(id.as_str())
        {
            return false;
        }
        for class in &self.classes {
            if !el.classes().any(|c| c == class) {
                return false;
            }
        }
        self.attrs.iter().all(|a| a.matches(el)) && self.pseudos.iter().all(|p| p.matches(el))
    }

    fn specificity(&self) -> Specificity {
        let mut s = (
            usize::from(self.id.is_some()),
            self.classes.len() + self.attrs.len(),
            usize::from(self.tag.as_deref().is_some_and(|t| t != "*")),
        );
        for p in &self.pseudos {
            let q = p.specificity();
            s = (s.0 + q.0, s.1 + q.1, s.2 + q.2);
        }
        s
    }
}

/// How a compound relates to the one on its right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comb {
    /// `a b`: any ancestor.
    Descendant,
    /// `a > b`: the parent.
    Child,
}

/// A selector: the compound for the element itself (the rightmost one, kept in
/// the flat fields) and, nearest first, the compounds it must have above it.
#[derive(Debug, Default, Clone)]
pub struct Selector {
    pub tag: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub attrs: Vec<AttrSel>,
    pub pseudos: Vec<Pseudo>,
    /// Compounds to the left, nearest first, with how each relates to the one to its right.
    pub ancestors: Vec<(Comb, Compound)>,
}

#[derive(Debug, Clone)]
pub struct Decl {
    pub name: String,
    pub value: String,
    pub important: bool,
}

/// CSS specificity as `(ids, classes, tags)`, compared lexicographically.
pub type Specificity = (usize, usize, usize);

impl Selector {
    pub fn specificity(&self) -> Specificity {
        let mut s = (
            usize::from(self.id.is_some()),
            self.classes.len() + self.attrs.len(),
            usize::from(self.tag.as_deref().is_some_and(|t| t != "*")),
        );
        for p in &self.pseudos {
            let q = p.specificity();
            s = (s.0 + q.0, s.1 + q.1, s.2 + q.2);
        }
        for (_, c) in &self.ancestors {
            let a = c.specificity();
            s = (s.0 + a.0, s.1 + a.1, s.2 + a.2);
        }
        s
    }

    /// Does the rightmost compound match `el`, ignoring the ancestors?
    pub fn matches_element(&self, el: &Element) -> bool {
        if let Some(t) = &self.tag
            && t != "*"
            && *t != el.tag
        {
            return false;
        }
        if let Some(id) = &self.id
            && el.id() != Some(id.as_str())
        {
            return false;
        }
        for class in &self.classes {
            if !el.classes().any(|c| c == class) {
                return false;
            }
        }
        self.attrs.iter().all(|a| a.matches(el)) && self.pseudos.iter().all(|p| p.matches(el))
    }

    /// Does this selector match `el`, whose ancestors are `anc` (outermost first,
    /// the parent last)? `budget` bounds the ancestor walking of a whole layout:
    /// when it runs out, selectors that need an ancestor stop matching.
    pub fn matches(&self, el: &Element, anc: &[&Element], budget: &mut u32) -> bool {
        if !self.matches_element(el) {
            return false;
        }
        let mut idx = anc.len();
        for (comb, comp) in &self.ancestors {
            match comb {
                Comb::Child => {
                    if idx == 0 || *budget == 0 {
                        return false;
                    }
                    *budget -= 1;
                    idx -= 1;
                    if !comp.matches(anc[idx]) {
                        return false;
                    }
                }
                Comb::Descendant => loop {
                    if idx == 0 || *budget == 0 {
                        return false;
                    }
                    *budget -= 1;
                    idx -= 1;
                    if comp.matches(anc[idx]) {
                        break;
                    }
                },
            }
        }
        true
    }
}

/// Drop `/* ... */` comments.
fn strip_comments(input: &str) -> String {
    if !input.contains("/*") {
        return String::from(input);
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        match rest[i + 2..].find("*/") {
            Some(j) => rest = &rest[i + 2 + j + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Parse a stylesheet. Tolerant: malformed rules and unsupported `@` rules are
/// skipped without aborting the parse. `@media` blocks keep their rules with the condition
/// ([`Media`]), checked against the viewport at layout time; `@supports` and `@layer` blocks are
/// read as if their content stood alone.
pub fn parse_css(input: &str) -> Stylesheet {
    let mut end = input.len().min(MAX_CSS_BYTES);
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    let text = strip_comments(&input[..end]);
    let mut st = ParseState::default();
    parse_into(text.as_bytes(), 0, 0, &mut st);
    Stylesheet {
        rules: st.rules,
        medias: st.medias,
        imports: st.imports,
        ..Default::default()
    }
}

#[derive(Default)]
struct ParseState {
    rules: Vec<Rule>,
    selector_total: usize,
    medias: Vec<Media>,
    imports: Vec<String>,
}

/// Deepest nesting of `@media` / `@supports` / `@layer` honoured.
const MAX_AT_DEPTH: u8 = 3;

/// Rules of `b` (the text of a sheet or of an `@media` block), tagged with `media`.
fn parse_into(b: &[u8], media: u16, depth: u8, st: &mut ParseState) {
    let mut i = 0;
    while i < b.len() {
        if st.rules.len() >= MAX_RULES || st.selector_total >= MAX_SELECTORS {
            break;
        }
        i = skip_css_ws(b, i);
        if i >= b.len() {
            break;
        }
        if b[i] == b'@' {
            let start = i;
            let end = skip_at_rule(b, i);
            let at = &b[start..end];
            let name_end = at[1..]
                .iter()
                .position(|c| !c.is_ascii_alphabetic() && *c != b'-')
                .map_or(at.len(), |p| p + 1);
            let name = core::str::from_utf8(&at[1..name_end])
                .unwrap_or("")
                .to_ascii_lowercase();
            let open = at.iter().position(|&c| c == b'{');
            match (name.as_str(), open) {
                ("import", None) => {
                    let prelude = core::str::from_utf8(&at[name_end..]).unwrap_or("");
                    if let Some(url) = import_url(prelude)
                        && st.imports.len() < 16
                    {
                        st.imports.push(url);
                    }
                }
                ("supports", Some(o))
                    if core::str::from_utf8(&at[name_end..o])
                        .unwrap_or("")
                        .trim_start()
                        .starts_with("not") => {}
                ("media" | "supports" | "layer", Some(o)) if depth < MAX_AT_DEPTH => {
                    let prelude = core::str::from_utf8(&at[name_end..o]).unwrap_or("");
                    let inner_end = at.iter().rposition(|&c| c == b'}').unwrap_or(at.len());
                    let inner = &at[(o + 1).min(inner_end)..inner_end];
                    let m = match name.as_str() {
                        "media" => match Media::parse(prelude) {
                            Some(m) if st.medias.len() < usize::from(u16::MAX) - 1 => {
                                st.medias.push(m);
                                Some(st.medias.len() as u16)
                            }
                            _ => None,
                        },
                        // `@supports (...)`: every property we know is supported often enough.
                        // `@layer name { ... }`: the order of layers is not modelled.
                        _ => Some(media),
                    };
                    if let Some(m) = m {
                        // A nested `@media` inside another one needs both: keep the inner
                        // condition only when the outer is none (the common case).
                        let tag = if media != 0 && name == "media" {
                            media
                        } else {
                            m
                        };
                        parse_into(inner, tag, depth + 1, st);
                    }
                }
                _ => {}
            }
            i = end;
            continue;
        }
        // Selector list up to '{'.
        let sel_start = i;
        while i < b.len() && b[i] != b'{' && b[i] != b'}' {
            i += 1;
        }
        if i >= b.len() || b[i] == b'}' {
            // A stray '}' (the end of something unsupported): skip it and go on.
            if i < b.len() {
                i += 1;
                continue;
            }
            break;
        }
        let sel_text = core::str::from_utf8(&b[sel_start..i]).unwrap_or("");
        i += 1; // '{'
        let decl_start = i;
        // The block ends at the '}' outside quotes and parentheses (`url(data:...;base64,...)`).
        let (mut depth_p, mut quote) = (0i32, 0u8);
        while i < b.len() {
            let c = b[i];
            if quote != 0 {
                if c == b'\\' {
                    i += 1;
                } else if c == quote {
                    quote = 0;
                }
            } else {
                match c {
                    b'"' | b'\'' => quote = c,
                    b'(' => depth_p += 1,
                    b')' => depth_p = (depth_p - 1).max(0),
                    b'}' if depth_p == 0 => break,
                    _ => {}
                }
            }
            i += 1;
        }
        let decl_text = core::str::from_utf8(&b[decl_start..i.min(b.len())]).unwrap_or("");
        if i < b.len() {
            i += 1; // '}'
        }
        let mut selectors = parse_selectors(sel_text);
        // Keep only as many selectors as the sheet-wide budget has left.
        selectors.truncate(MAX_SELECTORS - st.selector_total);
        let decls = parse_decls(decl_text);
        if !selectors.is_empty() && !decls.is_empty() {
            st.selector_total += selectors.len();
            st.rules.push(Rule {
                selectors,
                decls,
                author: true,
                media,
            });
        }
    }
}

/// The address in `@import url(x.css) screen;` or `@import "x.css";` (screen-compatible imports only).
fn import_url(prelude: &str) -> Option<String> {
    let p = prelude.trim().trim_end_matches(';').trim();
    let (url, rest) = if let Some(r) = p.strip_prefix("url(") {
        let close = r.find(')')?;
        (r[..close].trim(), r[close + 1..].trim())
    } else {
        let q = p.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let r = &p[1..];
        let close = r.find(q)?;
        (&r[..close], r[close + 1..].trim())
    };
    let url = url.trim_matches(|c| c == '"' || c == '\'');
    // A condition after the address (`print`, `(min-width: ...)`): only when it can hold on a screen.
    if !rest.is_empty() && Media::parse(rest).is_none_or(|m| !m.matches(DEFAULT_W, DEFAULT_H)) {
        return None;
    }
    (!url.is_empty()).then(|| url.to_string())
}

fn skip_css_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

fn skip_at_rule(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i] != b'{' && b[i] != b';' {
        i += 1;
    }
    if i < b.len() && b[i] == b';' {
        return i + 1;
    }
    // Balanced block.
    let mut depth = 0;
    while i < b.len() {
        match b[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    i
}

/// Split a selector list at the commas outside parentheses (`a:not(.b, .c), d`).
fn split_commas(s: &str) -> Vec<&str> {
    let (mut depth, mut start) = (0i32, 0usize);
    let mut out = Vec::new();
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = (depth - 1).max(0),
            ',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

fn parse_selectors(text: &str) -> Vec<Selector> {
    let mut out = Vec::new();
    for part in split_commas(text) {
        let part = part.trim();
        if part.is_empty() || part.len() > MAX_SELECTOR_LEN {
            continue;
        }
        if let Some(sel) = parse_complex_selector(part) {
            out.push(sel);
        }
    }
    out
}

/// Split a complex selector into compounds and combinators and build the
/// [`Selector`]. `None` for anything unsupported.
fn parse_complex_selector(s: &str) -> Option<Selector> {
    let b = s.as_bytes();
    let mut parts: Vec<(Comb, Compound)> = Vec::new(); // (relation to the NEXT compound, compound)
    let mut i = 0;
    let mut pending = Comb::Descendant;
    while i < b.len() {
        match b[i] {
            c if c.is_ascii_whitespace() => {
                i += 1;
            }
            b'>' => {
                pending = Comb::Child;
                i += 1;
            }
            b'+' | b'~' => return None,
            _ => {
                let (comp, n) = parse_compound(&s[i..])?;
                i += n;
                if parts.len() >= MAX_COMPOUNDS {
                    return None;
                }
                parts.push((pending, comp));
                pending = Comb::Descendant;
            }
        }
    }
    // `pending` after the loop would be a dangling combinator ("a >"): invalid.
    if parts.is_empty() || (pending == Comb::Child) {
        return None;
    }
    // `parts[k].0` is the combinator between compound k-1 and compound k.
    let n = parts.len();
    let mut ancestors: Vec<(Comb, Compound)> = Vec::new();
    for k in (0..n - 1).rev() {
        ancestors.push((parts[k + 1].0, parts[k].1.clone()));
    }
    let last = parts.pop()?.1;
    Some(Selector {
        tag: last.tag,
        id: last.id,
        classes: last.classes,
        attrs: last.attrs,
        pseudos: last.pseudos,
        ancestors,
    })
}

/// Parse one compound at the start of `s`; returns it and the bytes used.
fn parse_compound(s: &str) -> Option<(Compound, usize)> {
    let b = s.as_bytes();
    let mut c = Compound::default();
    let mut i = 0;
    if i < b.len() && (b[i].is_ascii_alphabetic() || b[i] == b'*') {
        let start = i;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-' || b[i] == b'*') {
            i += 1;
        }
        c.tag = Some(s[start..i].to_ascii_lowercase());
    }
    while i < b.len() {
        match b[i] {
            b'.' | b'#' => {
                let kind = b[i];
                i += 1;
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-' || b[i] == b'_')
                {
                    i += 1;
                }
                let name = s[start..i].to_string();
                if name.is_empty() {
                    return None;
                }
                if kind == b'.' {
                    c.classes.push(name);
                } else {
                    c.id = Some(name);
                }
            }
            b'[' => {
                let close = s[i..].find(']')? + i;
                c.attrs.push(parse_attr(&s[i + 1..close])?);
                i = close + 1;
            }
            b':' => {
                i += parse_pseudo(&s[i..], &mut c)?;
            }
            c if c.is_ascii_whitespace() || c == b'>' => break,
            b'+' | b'~' | b',' => return None,
            _ => return None,
        }
    }
    if c.tag.is_none()
        && c.id.is_none()
        && c.classes.is_empty()
        && c.attrs.is_empty()
        && c.pseudos.is_empty()
    {
        return None;
    }
    Some((c, i))
}

/// A pseudo-class at the start of `s` (which begins with `:`), added to `c`; returns the bytes
/// used. `None` for pseudo-elements and for what a static page can never match (`:hover`...), so
/// the whole selector is dropped.
fn parse_pseudo(s: &str, c: &mut Compound) -> Option<usize> {
    let b = s.as_bytes();
    if b.get(1) == Some(&b':') {
        return None; // a pseudo-element
    }
    let mut n = 1;
    while n < b.len() && (b[n].is_ascii_alphanumeric() || b[n] == b'-') {
        n += 1;
    }
    let name = s[1..n].to_ascii_lowercase();
    // Arguments in parentheses.
    let mut arg: Option<&str> = None;
    if b.get(n) == Some(&b'(') {
        let mut depth = 0;
        let mut end = None;
        for (k, ch) in s[n..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(n + k);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end?;
        arg = Some(&s[n + 1..end]);
        n = end + 1;
    }
    let nth = |a: i32, b: i32, last: bool, of_type: bool| Pseudo::Nth {
        a,
        b,
        last,
        of_type,
    };
    let p = match (name.as_str(), arg) {
        ("root", None) if c.tag.is_none() => {
            c.tag = Some("html".to_string());
            return Some(n);
        }
        ("first-child", None) => nth(0, 1, false, false),
        ("last-child", None) => nth(0, 1, true, false),
        ("first-of-type", None) => nth(0, 1, false, true),
        ("last-of-type", None) => nth(0, 1, true, true),
        ("only-child", None) => Pseudo::OnlyChild,
        ("empty", None) => Pseudo::Empty,
        ("link" | "any-link", None) => Pseudo::Link,
        ("checked", None) => Pseudo::Checked,
        ("disabled", None) => Pseudo::Disabled,
        ("enabled", None) => Pseudo::Enabled,
        ("nth-child" | "nth-last-child" | "nth-of-type" | "nth-last-of-type", Some(a)) => {
            let (a, b) = parse_nth(a)?;
            nth(a, b, name.contains("last"), name.ends_with("of-type"))
        }
        ("not" | "is" | "where" | "matches", Some(list)) => {
            let mut comps = Vec::new();
            for part in split_commas(list) {
                let part = part.trim();
                let (comp, used) = parse_compound(part)?;
                if used != part.len() || comps.len() >= 8 {
                    return None;
                }
                comps.push(comp);
            }
            if name == "not" {
                Pseudo::Not(comps)
            } else {
                Pseudo::Any(comps)
            }
        }
        _ => return None,
    };
    c.pseudos.push(p);
    Some(n)
}

/// `an+b`, `odd`, `even`, `3`, `-n+2`.
fn parse_nth(s: &str) -> Option<(i32, i32)> {
    let t: String = s
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    match t.as_str() {
        "odd" => return Some((2, 1)),
        "even" => return Some((2, 0)),
        _ => {}
    }
    if let Some(npos) = t.find('n') {
        let a = match &t[..npos] {
            "" | "+" => 1,
            "-" => -1,
            v => v.parse().ok()?,
        };
        let rest = &t[npos + 1..];
        let b = if rest.is_empty() {
            0
        } else {
            rest.parse().ok()?
        };
        Some((a, b))
    } else {
        Some((0, t.parse().ok()?))
    }
}

fn parse_attr(inner: &str) -> Option<AttrSel> {
    let inner = inner.trim();
    let ops = [
        ("~=", AttrOp::Word),
        ("^=", AttrOp::Prefix),
        ("$=", AttrOp::Suffix),
        ("*=", AttrOp::Contains),
        ("=", AttrOp::Equals),
    ];
    for (tok, op) in ops {
        if let Some((n, v)) = inner.split_once(tok) {
            let name = n.trim().to_ascii_lowercase();
            if name.is_empty() {
                return None;
            }
            // `[a="b" i]` flags are ignored (matching is case-insensitive anyway).
            let v = v.trim();
            let v = v.split_once(" i").map_or(v, |(a, _)| a).trim();
            let v = v.trim_matches(|c| c == '"' || c == '\'');
            return Some(AttrSel {
                name,
                op,
                value: v.to_string(),
            });
        }
    }
    let name = inner.to_ascii_lowercase();
    if name.is_empty()
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return None;
    }
    Some(AttrSel {
        name,
        op: AttrOp::Exists,
        value: String::new(),
    })
}

/// The declarations of a block: split at the `;` that sit outside parentheses and quotes.
fn split_decls(text: &str) -> Vec<&str> {
    let b = text.as_bytes();
    let (mut start, mut depth, mut quote) = (0usize, 0i32, 0u8);
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            if c == b'\\' {
                i += 1;
            } else if c == quote {
                quote = 0;
            }
        } else {
            match c {
                b'"' | b'\'' => quote = c,
                b'(' => depth += 1,
                b')' => depth = (depth - 1).max(0),
                b';' if depth == 0 => {
                    out.push(&text[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

pub(crate) fn parse_decls(text: &str) -> Vec<Decl> {
    let text = if text.contains("/*") {
        strip_comments(text)
    } else {
        String::from(text)
    };
    let mut out = Vec::new();
    for chunk in split_decls(&text) {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        if let Some((name, value)) = chunk.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let mut value = value.trim();
            let mut important = false;
            if let Some(p) = value.rfind('!')
                && value[p + 1..].trim().eq_ignore_ascii_case("important")
            {
                important = true;
                value = value[..p].trim();
            }
            if !name.is_empty() && !value.is_empty() {
                out.push(Decl {
                    name,
                    value: value.to_string(),
                    important,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod css_tests;
