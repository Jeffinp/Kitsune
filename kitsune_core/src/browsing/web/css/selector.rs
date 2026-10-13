//! Selectors: attribute tests, pseudo-classes, compounds, combinators and how a selector matches an element.

use super::*;

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
    pub(super) fn matches(&self, el: &Element) -> bool {
        let Some(v) = el.attrs.get(self.name.as_str()) else {
            return false;
        };
        let (v, want) = (v.as_str(), self.value.as_str());
        let eq = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
        match self.op {
            AttrOp::Exists => true,
            AttrOp::Equals => eq(v, want),
            AttrOp::Word => v.split_ascii_whitespace().any(|w| eq(w, want)),
            AttrOp::Prefix => !want.is_empty() && v.get(..want.len()).is_some_and(|h| eq(h, want)),
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
    pub(super) fn matches(&self, el: &Element) -> bool {
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

    pub(super) fn specificity(&self) -> (usize, usize, usize) {
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
pub(super) fn nth_matches(a: i32, b: i32, pos: i32) -> bool {
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
    /// `::before` (1) or `::after` (2) at the end of the selector, 0 for none.
    pub pseudo_el: u8,
}

impl Compound {
    pub(super) fn matches(&self, el: &Element) -> bool {
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

    pub(super) fn specificity(&self) -> Specificity {
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
    /// `a + b`: the element right before.
    Adjacent,
    /// `a ~ b`: any element before, under the same parent.
    Sibling,
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
    /// `::before` (1) or `::after` (2): the rule styles that generated box of the element.
    pub pseudo_el: u8,
    /// Compounds to the left, nearest first, with how each relates to the one to its right.
    pub ancestors: Vec<(Comb, Compound)>,
    /// What the compounds that must be ancestors need (their tags, ids and classes), see [`bloom`].
    pub(crate) need: bloom::Bloom,
}

impl Selector {
    /// Work out [`Selector::need`] from the ancestors (call once the selector is complete).
    pub(super) fn finish(&mut self) {
        let mut need = bloom::Bloom::default();
        for (comb, c) in &self.ancestors {
            // Only the compounds joined by `>` or a space sit on the path; the ones joined by `+` or `~`
            // are siblings of something on it.
            if matches!(comb, Comb::Child | Comb::Descendant) {
                bloom::add_compound(&mut need, c);
            }
        }
        self.need = need;
    }

    /// [`Selector::matches`] after a quick test against the Bloom filter of the ancestors' path.
    pub(crate) fn matches_in(
        &self,
        el: &Element,
        anc: &[&Element],
        path: &bloom::Bloom,
        budget: &mut u32,
    ) -> bool {
        if !self.ancestors.is_empty() && !bloom::may_hold(path, &self.need) {
            return false;
        }
        self.matches(el, anc, budget)
    }

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
        // The element the next compound is to the left of (a sibling step moves it sideways).
        let mut cur: &Element = el;
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
                    cur = anc[idx];
                }
                Comb::Descendant => loop {
                    if idx == 0 || *budget == 0 {
                        return false;
                    }
                    *budget -= 1;
                    idx -= 1;
                    if comp.matches(anc[idx]) {
                        cur = anc[idx];
                        break;
                    }
                },
                Comb::Adjacent | Comb::Sibling => {
                    // The siblings are the children of the parent, `anc[idx - 1]`.
                    if idx == 0 || *budget == 0 {
                        return false;
                    }
                    *budget -= 1;
                    let parent = anc[idx - 1];
                    let Some(pos) = parent
                        .children
                        .iter()
                        .position(|n| matches!(n, Node::Element(e) if core::ptr::eq(e, cur)))
                    else {
                        return false;
                    };
                    let mut found = None;
                    for n in parent.children[..pos].iter().rev() {
                        let Node::Element(s) = n else { continue };
                        if *budget == 0 {
                            return false;
                        }
                        *budget -= 1;
                        if comp.matches(s) {
                            found = Some(s);
                            break;
                        }
                        if *comb == Comb::Adjacent {
                            return false;
                        }
                    }
                    match found {
                        Some(s) => cur = s,
                        None => return false,
                    }
                }
            }
        }
        true
    }
}
