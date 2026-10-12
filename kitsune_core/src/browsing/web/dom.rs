//! DOM tree and a tolerant HTML parser (also captures <style> CSS).

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

mod entities;
mod text;
mod tree;
pub(crate) use entities::*;
pub(crate) use text::*;
pub use tree::*;

// ---- DOM ----

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
pub const MAX_NODES: usize = 20_000;
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
    let mut nodes = p.parse_nodes(&mut Vec::new());
    annotate(&mut nodes, 0);
    (nodes, p.css)
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
                sib: Sib::default(),
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
            sib: Sib::default(),
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
#[cfg(test)]
mod html_tests;
