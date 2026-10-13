//! Cosmetic rules: the boxes ad scripts would fill, hidden by a short list of generic selectors.

use crate::browsing::web::Doc;

/// The generic selectors, with `display: none !important` (embedded from `data/cosmetic.css`).
pub const COSMETIC_CSS: &str = include_str!("data/cosmetic.css");
/// Most selectors the cosmetic sheet may hold (a test keeps the file under it).
pub const MAX_COSMETIC_SELECTORS: usize = 32;

/// Hide the ad boxes of `doc`: add the cosmetic sheet to its rules and return how many boxes it
/// hides (an element inside a hidden one is not counted again). The sheet is added after the page's
/// external sheets arrive or before they do, either way its `!important` wins over ordinary rules.
pub fn hide_ads(doc: &mut Doc) -> u32 {
    doc.hide_matching(COSMETIC_CSS)
}
