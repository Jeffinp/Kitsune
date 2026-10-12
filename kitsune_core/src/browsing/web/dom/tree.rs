//! The tree the parser builds: nodes, elements and where an element stands among its siblings.

use super::*;

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
    /// Where it stands among its siblings (for `:first-child`, `:nth-child()` and the like).
    pub sib: Sib,
}

/// Position of an element among the element children of its parent, 1-based.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sib {
    pub index: u16,
    pub count: u16,
    /// The same among siblings with its tag name.
    pub tindex: u16,
    pub tcount: u16,
}

/// Fill in [`Sib`] for every element below `nodes`.
pub(crate) fn annotate(nodes: &mut [Node], depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let count = nodes
        .iter()
        .filter(|n| matches!(n, Node::Element(_)))
        .count()
        .min(usize::from(u16::MAX));
    let mut index = 0usize;
    // Per tag: how many siblings carry it (a handful of names, so a small list is enough).
    let mut by_tag: Vec<(String, u16, u16)> = Vec::new(); // (tag, total, seen)
    for n in nodes.iter() {
        if let Node::Element(e) = n {
            if let Some(t) = by_tag.iter_mut().find(|t| t.0 == e.tag) {
                t.1 = t.1.saturating_add(1);
            } else if by_tag.len() < 64 {
                by_tag.push((e.tag.clone(), 1, 0));
            }
        }
    }
    for n in nodes.iter_mut() {
        if let Node::Element(e) = n {
            index += 1;
            let (tcount, tindex) = match by_tag.iter_mut().find(|t| t.0 == e.tag) {
                Some(t) => {
                    t.2 = t.2.saturating_add(1);
                    (t.1, t.2)
                }
                None => (1, 1),
            };
            e.sib = Sib {
                index: index.min(usize::from(u16::MAX)) as u16,
                count: count as u16,
                tindex,
                tcount,
            };
            annotate(&mut e.children, depth + 1);
        }
    }
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

/// A cheap identity for an element name on the stack of open elements (FNV-1a): the stack
/// only decides whether a stray close tag closes an ancestor, so it need not hold the names.
pub(crate) fn name_key(name: &str) -> u64 {
    name_key_bytes(name.as_bytes())
}

/// [`name_key`] of a name as it stands in the source, any case.
pub(crate) fn name_key_bytes(name: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in name {
        h = (h ^ u64::from(b.to_ascii_lowercase())).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// HTML elements that never have children (void elements).
pub(crate) fn is_void(tag: &str) -> bool {
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
