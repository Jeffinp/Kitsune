//! CSS parser: rules, selectors (type, class, id, attribute, descendant and
//! child combinators), specificity and declaration lists.
//!
//! Anything the engine cannot honour is dropped at parse time instead of being
//! approximated: a selector with a pseudo-class (`a:hover`), a sibling combinator
//! or a pseudo-element never matches, so it cannot restyle the wrong elements.

use super::dom::{Element, Node};

mod media;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
pub use media::{DEFAULT_H, DEFAULT_W, Media, set_viewport, viewport_h, viewport_w};

pub(crate) mod bloom;
mod decls;
mod selector;
mod selector_parse;
mod sheet_parse;
pub(crate) use decls::parse_decls;
pub use selector::*;
use selector_parse::*;
pub use sheet_parse::parse_css;
use sheet_parse::*;

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
    /// Some rule styles a `::before` or `::after`.
    pub has_pseudo: bool,
    /// The rules that can match an element, found by what it is called, its `id` and its classes: per
    /// name, the indices (in rule order) of the rules whose selectors end in that tag, id or class.
    /// Empty until [`build_index`].
    by_tag: alloc::collections::BTreeMap<String, Vec<u32>>,
    by_class: alloc::collections::BTreeMap<String, Vec<u32>>,
    by_id: alloc::collections::BTreeMap<String, Vec<u32>>,
    /// The rules that name nothing to index by (`*`, `[attr]`, `:root`...).
    generic: Vec<u32>,
    indexed: bool,
    /// The rules matched by each element during the layout in progress, so an element that is
    /// styled again (box measuring, pseudo-elements, a second pass) is not matched again. The key is
    /// the element's address and the pseudo-element kind; [`Stylesheet::clear_memo`] empties it at
    /// the start of every layout (the document or the viewport may have changed since).
    pub(crate) memo: core::cell::RefCell<alloc::collections::BTreeMap<(usize, u8), Memo>>,
    /// For an element's address, the Bloom filter of the path from the root down to it (itself
    /// included); emptied with `memo`.
    pub(crate) paths: core::cell::RefCell<alloc::collections::BTreeMap<usize, bloom::Bloom>>,
}
/// The memoized matches of one element: the rules (index into `rules`) with their specificity, in
/// cascade order, and the ancestor path they were found for (its length and its last element).
#[derive(Debug)]
pub(crate) struct Memo {
    pub(crate) depth: usize,
    pub(crate) parent: usize,
    pub(crate) hits: alloc::rc::Rc<Vec<(Specificity, u32)>>,
}
impl Stylesheet {
    /// Forget the memoized matches (see [`Stylesheet::memo`]).
    pub fn clear_memo(&self) {
        self.memo.borrow_mut().clear();
        self.paths.borrow_mut().clear();
    }

    /// The Bloom filter of the path `anc` (outermost first): every ancestor's tag, id and classes.
    pub(crate) fn path_bloom(&self, anc: &[&Element]) -> bloom::Bloom {
        let mut paths = self.paths.borrow_mut();
        // The deepest ancestor whose filter is already known; the rest are folded in from there.
        let mut from = 0;
        let mut f = bloom::Bloom::default();
        for (i, a) in anc.iter().enumerate().rev() {
            if let Some(known) = paths.get(&(*a as *const Element as usize)) {
                f = *known;
                from = i + 1;
                break;
            }
        }
        for a in &anc[from..] {
            bloom::add_element(&mut f, a);
            paths.insert(*a as *const Element as usize, f);
        }
        f
    }

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
    /// `::before` (1) / `::after` (2): the rule styles the generated box, not the element (0 = the element).
    pub pseudo: u8,
    /// `1 + index` into [`Stylesheet::medias`] when the rule sits in an `@media` block (0 = always).
    pub media: u16,
}
#[derive(Debug, Clone)]
pub struct Decl {
    pub name: String,
    pub value: String,
    pub important: bool,
}
/// CSS specificity as `(ids, classes, tags)`, compared lexicographically.
pub type Specificity = (usize, usize, usize);
#[cfg(test)]
mod css_tests;
