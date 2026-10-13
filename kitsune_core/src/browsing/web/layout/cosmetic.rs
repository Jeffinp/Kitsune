//! Hiding boxes by selector (the blocker's cosmetic rules).

use super::super::css::Selector;
use super::super::dom::MAX_DEPTH;
use super::*;

impl Doc {
    /// Add `css` (a style sheet of `display: none` rules) to the page and count the elements its
    /// selectors hide: the outermost matches only, so a hidden box and what it holds count once.
    /// Selectors are tested on the element alone (no ancestors), which is what generic "ad box"
    /// selectors need; the count walks at most [`MAX_NODES`](crate::web::MAX_NODES) elements deep
    /// down to [`MAX_DEPTH`](crate::web::MAX_DEPTH).
    pub fn hide_matching(&mut self, css: &str) -> u32 {
        let parsed = parse_css(css);
        let selectors: Vec<&Selector> = parsed
            .rules
            .iter()
            .flat_map(|r| r.selectors.iter())
            .filter(|s| s.ancestors.is_empty() && s.pseudo_el == 0)
            .collect();
        let hidden = count_hidden(&self.dom, &selectors, 0);
        // Also hide through the cascade, so layout skips the boxes and their pictures.
        self.add_sheet(css);
        hidden
    }
}

fn count_hidden(nodes: &[Node], selectors: &[&Selector], depth: usize) -> u32 {
    if depth > MAX_DEPTH {
        return 0;
    }
    let mut n = 0u32;
    for node in nodes {
        let Node::Element(e) = node else { continue };
        if selectors.iter().any(|s| s.matches_element(e)) {
            n = n.saturating_add(1);
        } else {
            n = n.saturating_add(count_hidden(&e.children, selectors, depth + 1));
        }
    }
    n
}
