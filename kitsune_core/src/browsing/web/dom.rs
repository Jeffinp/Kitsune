//! DOM tree and a tolerant HTML parser (also captures <style> CSS).

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ---- DOM ----

/// A DOM node: either an element or a run of text.
#[derive(Debug)]
pub enum Node {
    Element(Element),
    Text(String),
}

/// An element node with its tag, attributes and children.
#[derive(Debug)]
pub struct Element {
    pub tag: String,
    pub attrs: BTreeMap<String, String>,
    pub children: Vec<Node>,
    /// Whitespace-only text sat between this element and the previous node (so
    /// `<b>a</b> <i>b</i>` keeps its space without a text node costing budget).
    pub ws_before: bool,
}

/// A cheap identity for an element name on the stack of open elements (FNV-1a): the stack
/// only decides whether a stray close tag closes an ancestor, so it need not hold the names.
fn name_key(name: &str) -> u64 {
    name_key_bytes(name.as_bytes())
}

/// [`name_key`] of a name as it stands in the source, any case.
fn name_key_bytes(name: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in name {
        h = (h ^ u64::from(b.to_ascii_lowercase())).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

impl Element {
    pub fn id(&self) -> Option<&str> {
        self.attrs.get("id").map(|s| s.as_str())
    }
    pub fn classes(&self) -> impl Iterator<Item = &str> {
        self.attrs
            .get("class")
            .map(|s| s.as_str())
            .unwrap_or("")
            .split_ascii_whitespace()
    }
}

/// HTML elements that never have children (void elements).
fn is_void(tag: &str) -> bool {
    matches!(
        tag,
        "br" | "img"
            | "hr"
            | "meta"
            | "link"
            | "input"
            | "area"
            | "base"
            | "col"
            | "embed"
            | "source"
            | "track"
            | "wbr"
    )
}

/// Maximum element nesting depth kept in the DOM (see `parse_element`). Every
/// level costs roughly 550 bytes of native stack across parse + layout in a
/// release build, so 40 levels is ~22 KiB — far below the kernel stack, yet
/// deeper than real-world page structure under `<body>`.
pub const MAX_DEPTH: usize = 40;

/// Maximum number of DOM nodes (elements + text runs) kept for one page.
///
/// Style resolution is O(rules x elements) and the display list grows with
/// the node count, so an unbounded DOM lets a 256 KiB response (one `<i>` is
/// 3 bytes) cost seconds of CPU and megabytes of heap. Parsing stops once the
/// budget is spent: the page is rendered truncated, never refused. Real pages
/// under `<body>` stay well below this.
pub const MAX_NODES: usize = 8_000;

/// Parse an HTML document into a DOM tree plus the concatenated text of every
/// `<style>` element (the page's CSS). Tolerant of malformed markup: unknown
/// tags pass through, mismatched close tags pop to the nearest match.
pub fn parse_html(input: &[u8]) -> (Vec<Node>, String) {
    // A page that is not valid UTF-8 is read as Windows-1252 (what pages without a
    // declared charset almost always are) instead of rendering as nothing.
    let converted;
    let s = match core::str::from_utf8(input) {
        Ok(s) => s,
        Err(_) => {
            converted = latin1_to_string(input);
            converted.as_str()
        }
    };
    let mut p = HtmlParser {
        b: s.as_bytes(),
        i: 0,
        css: String::new(),
        nodes: 0,
        pending_ws: false,
        pre_depth: 0,
    };
    let nodes = p.parse_nodes(&mut Vec::new());
    (nodes, p.css)
}

/// Decode Windows-1252 bytes (Latin-1 plus the printable 0x80-0x9F block).
pub(crate) fn latin1_to_string(b: &[u8]) -> String {
    crate::format::text::windows1252_to_string(b)
}

struct HtmlParser<'a> {
    b: &'a [u8],
    i: usize,
    css: String,
    /// Nodes created so far (see [`MAX_NODES`]).
    nodes: usize,
    /// Whitespace-only text was just skipped (it becomes the next element's `ws_before`).
    pending_ws: bool,
    /// Open `<pre>` elements: whitespace-only text is kept verbatim inside them.
    pre_depth: usize,
}

impl HtmlParser<'_> {
    fn eof(&self) -> bool {
        self.i >= self.b.len()
    }
    fn peek(&self) -> u8 {
        self.b[self.i]
    }
    fn starts_with(&self, s: &[u8]) -> bool {
        self.b[self.i..].starts_with(s)
    }
    /// Is the next thing the start tag `<name` (any case, followed by a name end)?
    fn at_open_tag(&self, name: &[u8]) -> bool {
        let r = &self.b[self.i..];
        r.len() > name.len() + 1
            && r[0] == b'<'
            && r[1..=name.len()].eq_ignore_ascii_case(name)
            && !(r[name.len() + 1].is_ascii_alphanumeric() || r[name.len() + 1] == b'-')
    }

    /// Parse sibling nodes until EOF or an unmatched close tag whose name is on
    /// the `open` stack (so the caller can pop to it).
    fn parse_nodes(&mut self, open: &mut Vec<u64>) -> Vec<Node> {
        let mut nodes = Vec::new();
        while !self.eof() {
            if self.nodes >= MAX_NODES {
                // Node budget spent: drop the rest of the document. Every
                // enclosing `parse_nodes` then sees EOF and unwinds normally.
                self.i = self.b.len();
                break;
            }
            if self.starts_with(b"<!--") {
                self.skip_comment();
                continue;
            }
            if self.starts_with(b"<!") {
                self.skip_until(b'>');
                continue;
            }
            if self.starts_with(b"</") {
                // A close tag: stop so the matching opener can consume it.
                break;
            }
            if self.peek() == b'<' {
                // `<option>` and `<optgroup>` have optional end tags: one that starts while an
                // option is open ends it (a long list would otherwise nest past the depth limit).
                if open.last() == Some(&name_key("option"))
                    && (self.at_open_tag(b"option") || self.at_open_tag(b"optgroup"))
                {
                    break;
                }
                if let Some(node) = self.parse_element(open) {
                    nodes.push(node);
                }
            } else {
                let text = self.parse_text();
                if text.is_empty() {
                    continue;
                }
                if self.pre_depth == 0 && text.bytes().all(is_html_space) {
                    self.pending_ws = true;
                } else {
                    self.nodes += 1;
                    self.pending_ws = false;
                    nodes.push(Node::Text(text));
                }
            }
        }
        nodes
    }

    fn parse_text(&mut self) -> String {
        let start = self.i;
        while !self.eof() && self.peek() != b'<' {
            self.i += 1;
        }
        decode_text(core::str::from_utf8(&self.b[start..self.i]).unwrap_or(""))
    }

    fn parse_element(&mut self, open: &mut Vec<u64>) -> Option<Node> {
        // '<'
        self.i += 1;
        let tag = self.parse_name_lower();
        if tag.is_empty() {
            self.skip_until(b'>');
            return None;
        }
        let attrs = self.parse_attrs();
        let self_closing = self.consume_tag_end();
        let ws_before = core::mem::take(&mut self.pending_ws);

        // <script>/<style>: consume raw text up to the matching close tag.
        if tag == "script" || tag == "style" {
            let raw = self.read_raw_until_close(&tag);
            if tag == "style" {
                self.css.push_str(&raw);
                self.css.push('\n');
            }
            return None;
        }

        if self_closing || is_void(&tag) {
            self.nodes += 1;
            return Some(Node::Element(Element {
                tag,
                attrs,
                children: Vec::new(),
                ws_before,
            }));
        }

        // Bound the nesting depth. The parser, the style/layout passes and the
        // tree's `Drop` are all recursive, and a page controls the depth: ~150
        // nested elements already overflowed the kernel's 80 KiB stack. Past
        // the limit the wrapper is dropped but its content stays in the parent.
        if open.len() >= MAX_DEPTH {
            return None;
        }

        self.nodes += 1;
        let tag_key = name_key(&tag);
        open.push(tag_key);
        let is_pre = tag == "pre";
        if is_pre {
            self.pre_depth += 1;
            // A newline right after `<pre>` is not content.
            if self.starts_with(b"\r\n") {
                self.i += 2;
            } else if !self.eof() && self.peek() == b'\n' {
                self.i += 1;
            }
        }
        let children = self.parse_nodes(open);
        if is_pre {
            self.pre_depth -= 1;
        }
        // Consume the matching close tag if present.
        if self.starts_with(b"</") {
            let save = self.i;
            self.i += 2;
            let (ca, cb) = self.name_range();
            let close = &self.b[ca..cb];
            let same = close.eq_ignore_ascii_case(tag.as_bytes());
            let key = name_key_bytes(close);
            self.skip_until(b'>');
            // Mismatched close tag that an ancestor opened: rewind so it can
            // handle the close (auto-closing this element).
            if !same && open.contains(&key) {
                self.i = save;
            }
        }
        open.pop();
        Some(Node::Element(Element {
            tag,
            attrs,
            children,
            ws_before,
        }))
    }

    /// Skip a name (letters, digits, `-`, `_`, `:`) and return its byte range.
    fn name_range(&mut self) -> (usize, usize) {
        let start = self.i;
        while !self.eof() {
            let c = self.peek();
            if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b':' {
                self.i += 1;
            } else {
                break;
            }
        }
        (start, self.i)
    }

    /// A name, lower case. Names are ASCII, so one allocation of the exact size.
    fn parse_name_lower(&mut self) -> String {
        let (a, b) = self.name_range();
        let mut v = Vec::with_capacity(b - a);
        v.extend(self.b[a..b].iter().map(u8::to_ascii_lowercase));
        String::from_utf8(v).unwrap_or_default()
    }

    fn parse_attrs(&mut self) -> BTreeMap<String, String> {
        let mut attrs = BTreeMap::new();
        loop {
            self.skip_ws();
            if self.eof() || self.peek() == b'>' || self.starts_with(b"/>") {
                break;
            }
            let name = self.parse_name_lower();
            if name.is_empty() {
                // Stray character (e.g. a lone '/'): skip it to make progress.
                self.i += 1;
                continue;
            }
            self.skip_ws();
            let value = if !self.eof() && self.peek() == b'=' {
                self.i += 1;
                self.skip_ws();
                self.parse_attr_value()
            } else {
                String::new()
            };
            attrs.insert(name, value);
        }
        attrs
    }

    fn parse_attr_value(&mut self) -> String {
        if self.eof() {
            return String::new();
        }
        let q = self.peek();
        if q == b'"' || q == b'\'' {
            self.i += 1;
            let start = self.i;
            while !self.eof() && self.peek() != q {
                self.i += 1;
            }
            let v = String::from_utf8_lossy(&self.b[start..self.i]).into_owned();
            if !self.eof() {
                self.i += 1; // closing quote
            }
            v
        } else {
            let start = self.i;
            while !self.eof() && !self.peek().is_ascii_whitespace() && self.peek() != b'>' {
                self.i += 1;
            }
            String::from_utf8_lossy(&self.b[start..self.i]).into_owned()
        }
    }

    /// Consume the `>` (or `/>`) that ends a start tag. Returns true if it was
    /// self-closing.
    fn consume_tag_end(&mut self) -> bool {
        self.skip_ws();
        let mut self_closing = false;
        if self.starts_with(b"/>") {
            self_closing = true;
            self.i += 2;
        } else if !self.eof() && self.peek() == b'>' {
            self.i += 1;
        } else {
            self.skip_until(b'>');
        }
        self_closing
    }

    fn read_raw_until_close(&mut self, tag: &str) -> String {
        let start = self.i;
        let needle_lower = alloc::format!("</{tag}");
        loop {
            if self.eof() {
                let raw = String::from_utf8_lossy(&self.b[start..self.i]).into_owned();
                return raw;
            }
            if self.b[self.i..].len() >= needle_lower.len() {
                let win = &self.b[self.i..self.i + needle_lower.len()];
                if win.eq_ignore_ascii_case(needle_lower.as_bytes()) {
                    let raw = String::from_utf8_lossy(&self.b[start..self.i]).into_owned();
                    self.skip_until(b'>');
                    return raw;
                }
            }
            self.i += 1;
        }
    }

    fn skip_ws(&mut self) {
        while !self.eof() && self.peek().is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn skip_until(&mut self, ch: u8) {
        while !self.eof() && self.peek() != ch {
            self.i += 1;
        }
        if !self.eof() {
            self.i += 1;
        }
    }
    fn skip_comment(&mut self) {
        self.i += 4; // <!--
        while !self.eof() && !self.starts_with(b"-->") {
            self.i += 1;
        }
        self.i = (self.i + 3).min(self.b.len());
    }
}

/// HTML's whitespace: space, tab, line feed, form feed, carriage return.
pub(crate) fn is_html_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0c | b'\r')
}

/// May `c` appear in page text? Controls, bidirectional overrides (they can make
/// text lie about its order), zero-width joiners, combining marks (the face has no
/// composition) and non-characters are dropped. A zero-width space stays: it is
/// where a long word may break.
pub(crate) fn keep_char(c: char) -> bool {
    !matches!(
        c,
        '\u{0}'..='\u{8}'
            | '\u{b}'
            | '\u{e}'..='\u{1f}'
            | '\u{7f}'..='\u{9f}'
            | '\u{200c}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{061c}'
            | '\u{feff}'
            | '\u{fe00}'..='\u{fe0f}'
            | '\u{0300}'..='\u{036f}'
            | '\u{fffe}'
            | '\u{ffff}'
    )
}

/// Decode HTML entities and sanitise `text`; whitespace is kept as written
/// (line ends normalised to `\n`) because collapsing depends on the style of the
/// element the text ends up in (`white-space: pre`), which layout knows.
fn decode_text(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'&' {
            let mut j = i + 1;
            while j < bytes.len() && j < i + 12 && bytes[j] != b';' {
                j += 1;
            }
            if j < bytes.len()
                && bytes[j] == b';'
                && let Some(ch) = text.get(i + 1..j).and_then(entity_char)
            {
                if keep_char(ch) {
                    out.push(ch);
                }
                i = j + 1;
                continue;
            }
            out.push('&');
            i += 1;
        } else if c == b'\r' {
            // CRLF and lone CR are line ends.
            out.push('\n');
            i += 1;
            if i < bytes.len() && bytes[i] == b'\n' {
                i += 1;
            }
        } else if c == 0x0c {
            out.push(' ');
            i += 1;
        } else if c < 0x80 {
            if keep_char(c as char) {
                out.push(c as char);
            }
            i += 1;
        } else {
            // `i` is on a char boundary: only whole characters are skipped.
            let ch = text[i..].chars().next().unwrap_or('\u{fffd}');
            if keep_char(ch) {
                out.push(ch);
            }
            i += ch.len_utf8();
        }
    }
    out
}

/// Named entities that decode to one character (the Latin letters Portuguese
/// pages use, common punctuation and the markup-significant ones).
fn entity_char(name: &str) -> Option<char> {
    const NAMED: &[(&str, char)] = &[
        ("aacute", '\u{e1}'),
        ("agrave", '\u{e0}'),
        ("acirc", '\u{e2}'),
        ("atilde", '\u{e3}'),
        ("auml", '\u{e4}'),
        ("aring", '\u{e5}'),
        ("aelig", '\u{e6}'),
        ("eacute", '\u{e9}'),
        ("egrave", '\u{e8}'),
        ("ecirc", '\u{ea}'),
        ("euml", '\u{eb}'),
        ("iacute", '\u{ed}'),
        ("igrave", '\u{ec}'),
        ("icirc", '\u{ee}'),
        ("iuml", '\u{ef}'),
        ("oacute", '\u{f3}'),
        ("ograve", '\u{f2}'),
        ("ocirc", '\u{f4}'),
        ("otilde", '\u{f5}'),
        ("ouml", '\u{f6}'),
        ("oslash", '\u{f8}'),
        ("uacute", '\u{fa}'),
        ("ugrave", '\u{f9}'),
        ("ucirc", '\u{fb}'),
        ("uuml", '\u{fc}'),
        ("yacute", '\u{fd}'),
        ("ccedil", '\u{e7}'),
        ("ntilde", '\u{f1}'),
        ("szlig", '\u{df}'),
        ("Aacute", '\u{c1}'),
        ("Agrave", '\u{c0}'),
        ("Acirc", '\u{c2}'),
        ("Atilde", '\u{c3}'),
        ("Auml", '\u{c4}'),
        ("Aring", '\u{c5}'),
        ("AElig", '\u{c6}'),
        ("Eacute", '\u{c9}'),
        ("Egrave", '\u{c8}'),
        ("Ecirc", '\u{ca}'),
        ("Euml", '\u{cb}'),
        ("Iacute", '\u{cd}'),
        ("Igrave", '\u{cc}'),
        ("Icirc", '\u{ce}'),
        ("Iuml", '\u{cf}'),
        ("Oacute", '\u{d3}'),
        ("Ograve", '\u{d2}'),
        ("Ocirc", '\u{d4}'),
        ("Otilde", '\u{d5}'),
        ("Ouml", '\u{d6}'),
        ("Oslash", '\u{d8}'),
        ("Uacute", '\u{da}'),
        ("Ugrave", '\u{d9}'),
        ("Ucirc", '\u{db}'),
        ("Uuml", '\u{dc}'),
        ("Yacute", '\u{dd}'),
        ("Ccedil", '\u{c7}'),
        ("Ntilde", '\u{d1}'),
        ("euro", '\u{20ac}'),
        ("copy", '\u{a9}'),
        ("reg", '\u{ae}'),
        ("trade", '\u{2122}'),
        ("ndash", '\u{2013}'),
        ("mdash", '\u{2014}'),
        ("hellip", '\u{2026}'),
        ("lsquo", '\u{2018}'),
        ("rsquo", '\u{2019}'),
        ("ldquo", '\u{201c}'),
        ("rdquo", '\u{201d}'),
        ("bull", '\u{2022}'),
        ("middot", '\u{b7}'),
        ("laquo", '\u{ab}'),
        ("raquo", '\u{bb}'),
        ("times", '\u{d7}'),
        ("divide", '\u{f7}'),
        ("deg", '\u{b0}'),
        ("plusmn", '\u{b1}'),
        ("sect", '\u{a7}'),
        ("para", '\u{b6}'),
        ("cent", '\u{a2}'),
        ("pound", '\u{a3}'),
        ("yen", '\u{a5}'),
        ("iexcl", '\u{a1}'),
        ("iquest", '\u{bf}'),
        ("ordf", '\u{aa}'),
        ("ordm", '\u{ba}'),
        ("sup2", '\u{b2}'),
        ("sup3", '\u{b3}'),
        ("micro", '\u{b5}'),
        ("frac12", '\u{bd}'),
        ("frac14", '\u{bc}'),
        ("frac34", '\u{be}'),
        ("larr", '\u{2190}'),
        ("uarr", '\u{2191}'),
        ("rarr", '\u{2192}'),
        ("darr", '\u{2193}'),
        ("minus", '\u{2212}'),
    ];
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{a0}'),
        _ => {
            if let [b'#', rest @ ..] = name.as_bytes() {
                let cp = if let [b'x' | b'X', hex @ ..] = rest {
                    u32::from_str_radix(core::str::from_utf8(hex).ok()?, 16).ok()?
                } else {
                    core::str::from_utf8(rest).ok()?.parse::<u32>().ok()?
                };
                // Controls would let a page smuggle line breaks into a URL or field.
                if cp < 0x20 || (0x7f..0xa0).contains(&cp) {
                    return None;
                }
                return char::from_u32(cp);
            }
            NAMED.iter().find(|(n, _)| *n == name).map(|&(_, c)| c)
        }
    }
}

/// Decode the entities of an attribute value (`&amp;` in a URL, `&eacute;` in a
/// field value) to real characters, keeping every other character (UTF-8
/// included) as it is.
pub(crate) fn decode_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'&' {
            let mut j = i + 1;
            while j < b.len() && j < i + 12 && b[j] != b';' {
                j += 1;
            }
            if j < b.len()
                && b[j] == b';'
                && let Some(ch) = s.get(i + 1..j).and_then(entity_char)
            {
                out.push(ch);
                i = j + 1;
                continue;
            }
        }
        // `i` always sits on a char boundary: it only advances by whole characters.
        let ch = s[i..].chars().next().unwrap_or('\u{fffd}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// `s` made fit for one line of display: runs of whitespace and controls collapse to
/// one space and the characters [`keep_char`] refuses are dropped. Accents and every
/// other character are kept.
pub(crate) fn fold_display(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for ch in s.chars() {
        let cp = ch as u32;
        if (ch.is_whitespace() && ch != '\u{a0}') || cp < 0x20 || cp == 0x7f {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else if keep_char(ch) {
            out.push(ch);
            last_space = false;
        }
    }
    out
}

/// The text of `nodes` (depth-limited), without markup.
pub(crate) fn text_content(nodes: &[Node]) -> String {
    fn walk(nodes: &[Node], depth: usize, out: &mut String) {
        if depth > 12 || out.len() > 4096 {
            return;
        }
        for n in nodes {
            match n {
                Node::Text(t) => out.push_str(t),
                Node::Element(e) => walk(&e.children, depth + 1, out),
            }
        }
    }
    let mut s = String::new();
    walk(nodes, 0, &mut s);
    s
}

#[cfg(test)]
mod html_tests;
