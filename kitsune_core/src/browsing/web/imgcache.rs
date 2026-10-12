//! Images in pages: what the layout needs to know about each `<img>`
//! ([`ImageLookup`]), the decoder that turns a downloaded body into a pixel
//! buffer within the page's budgets ([`decode_for_page`]) and the small LRU
//! cache that holds the results ([`ImageCache`]).
//!
//! The network part lives in the kernel (`fetch.rs`): it downloads one image at
//! a time on the background `fetcher` thread, calls [`decode_for_page`] there
//! (so the UI never waits for a decode) and hands the result to the compositor,
//! which puts it in the cache and lays the page out again. Everything decided
//! here is pure and tested on the host.
//!
//! # Limits
//!
//! | what | limit |
//! |---|---|
//! | images fetched per page | [`MAX_PAGE_IMAGES`] = 8 (the rest show "limite de imagens") |
//! | bytes downloaded per image | [`MAX_IMAGE_BYTES`] = 512 KiB |
//! | pixels decoded | [`MAX_DECODE_PIXELS`] = 2 Mpx (8 MiB of RGBA), checked on the header |
//! | `data:` image payload | [`MAX_DATA_IMAGE_BYTES`] = 64 KiB |
//! | pixel memory kept | [`CACHE_BYTES`] = 6 MiB, least recently used goes first |
//!
//! A decoded image is scaled to the page's column width on the spot
//! ([`crate::format::image::Image::fit`]) and flattened over the page background, so
//! the cache keeps one opaque, already-small buffer per image.

use crate::format::base64;
use crate::format::image::{self, Filter, Format, Image};
use crate::format::svg;
use crate::tk;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::Cell;

/// Images fetched for one page; later ones are shown as "limite de imagens".
pub const MAX_PAGE_IMAGES: usize = 8;
/// Largest response body accepted for one image.
pub const MAX_IMAGE_BYTES: usize = 512 * 1024;
/// Largest decoded size (width x height) before any pixel memory is reserved.
pub const MAX_DECODE_PIXELS: usize = 2 * 1024 * 1024;
/// Decoded payload limit for a `data:` image.
pub const MAX_DATA_IMAGE_BYTES: usize = 64 * 1024;
/// Longest `data:` URI kept in a page (`src` attribute), in bytes.
pub const MAX_DATA_URI_LEN: usize = 96 * 1024;
/// Pixel memory the cache may hold (RGBA, 4 bytes per pixel).
pub const CACHE_BYTES: usize = 6 * 1024 * 1024;
/// Entries kept (loaded or not).
pub const MAX_ENTRIES: usize = 24;
/// Background the pictures are flattened over (the page's white-ish).
pub const PAGE_BG: u32 = 0xFFF7_F9FC;

/// What the layout may know about an image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImgState {
    /// Not downloaded yet: the layout reserves a grey box.
    Pending,
    /// Decoded; `w`x`h` are the picture's own (original) dimensions.
    Ready { w: i32, h: i32 },
    /// Not a PNG/BMP/PPM (JPEG, GIF, WebP, SVG, ...).
    Unsupported,
    /// Could not be downloaded or decoded.
    Failed,
    /// Over the byte or pixel limit.
    TooBig,
    /// Past [`MAX_PAGE_IMAGES`].
    TooMany,
}

impl ImgState {
    /// Catalog key of the line shown under the alt text of a box that has no picture;
    /// `None` for pending / ready.
    pub fn message_key(self) -> Option<&'static str> {
        match self {
            ImgState::Pending | ImgState::Ready { .. } => None,
            ImgState::Unsupported => Some(tk!("web.img.unsupported")),
            ImgState::Failed => Some(tk!("web.img.failed")),
            ImgState::TooBig => Some(tk!("web.img.too_big")),
            ImgState::TooMany => Some(tk!("web.img.too_many")),
        }
    }

    /// The line in the language in effect (the layout bakes it into the page, so the kernel
    /// lays a page out again when the language changes).
    pub fn message(self) -> Option<&'static str> {
        self.message_key().map(crate::i18n::tr)
    }
}

/// How the layout asks about an image by its `src` attribute (as written in
/// the page: the resolver turns it into a key).
pub trait ImageLookup {
    fn lookup(&self, src: &str) -> ImgState;
}

/// A lookup that knows no images: everything is [`ImgState::Pending`].
pub struct NoImages;

impl ImageLookup for NoImages {
    fn lookup(&self, _src: &str) -> ImgState {
        ImgState::Pending
    }
}

/// Why an image could not be turned into pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImgFail {
    Unsupported,
    TooBig,
    Failed,
}

impl ImgFail {
    fn state(self) -> ImgState {
        match self {
            ImgFail::Unsupported => ImgState::Unsupported,
            ImgFail::TooBig => ImgState::TooBig,
            ImgFail::Failed => ImgState::Failed,
        }
    }
}

/// A decoded, column-sized, opaque picture plus its original dimensions.
pub struct Loaded {
    pub orig_w: usize,
    pub orig_h: usize,
    pub img: Image,
}

/// Width and height from the file header, without decoding. `None` for an
/// unknown or truncated header.
pub fn peek_dims(bytes: &[u8]) -> Option<(usize, usize)> {
    if svg::looks_like(bytes) {
        return svg::dims(bytes);
    }
    match image::detect(bytes)? {
        Format::Png => {
            let h = crate::format::png::read_header(bytes).ok()?;
            Some((h.width as usize, h.height as usize))
        }
        Format::Bmp => {
            let hs = u32::from_le_bytes(bytes.get(14..18)?.try_into().ok()?);
            if hs == 12 {
                let w = u16::from_le_bytes(bytes.get(18..20)?.try_into().ok()?);
                let h = u16::from_le_bytes(bytes.get(20..22)?.try_into().ok()?);
                Some((usize::from(w), usize::from(h)))
            } else {
                let w = i32::from_le_bytes(bytes.get(18..22)?.try_into().ok()?);
                let h = i32::from_le_bytes(bytes.get(22..26)?.try_into().ok()?);
                Some((w.unsigned_abs() as usize, h.unsigned_abs() as usize))
            }
        }
        Format::Gif => crate::format::gif::peek_dims(bytes),
        Format::Jpeg => crate::format::jpeg::peek_dims(bytes),
        Format::Ppm => {
            let mut pos = 2;
            let mut nums = [0usize; 2];
            for n in &mut nums {
                loop {
                    match bytes.get(pos)? {
                        b'#' => {
                            while *bytes.get(pos)? != b'\n' {
                                pos += 1;
                            }
                        }
                        b if b.is_ascii_whitespace() => pos += 1,
                        _ => break,
                    }
                }
                let mut v = 0usize;
                let start = pos;
                while let Some(d) = bytes.get(pos).filter(|b| b.is_ascii_digit()) {
                    v = v.checked_mul(10)?.checked_add(usize::from(d - b'0'))?;
                    pos += 1;
                }
                if pos == start {
                    return None;
                }
                *n = v;
            }
            Some((nums[0], nums[1]))
        }
    }
}

/// Decode a downloaded body for a page whose column is `fit_w` pixels wide.
/// Checks the header against [`MAX_DECODE_PIXELS`] before reserving memory,
/// scales down to `fit_w` and flattens over [`PAGE_BG`]. Never panics.
pub fn decode_for_page(body: &[u8], fit_w: usize) -> Result<Loaded, ImgFail> {
    if body.len() > MAX_IMAGE_BYTES {
        return Err(ImgFail::TooBig);
    }
    if svg::looks_like(body) {
        return decode_svg(body, fit_w);
    }
    if image::detect(body).is_none() {
        return Err(ImgFail::Unsupported);
    }
    let (w, h) = peek_dims(body).ok_or(ImgFail::Failed)?;
    if w == 0 || h == 0 {
        return Err(ImgFail::Failed);
    }
    if w.checked_mul(h).is_none_or(|n| n > MAX_DECODE_PIXELS) {
        return Err(ImgFail::TooBig);
    }
    let img = image::decode(body).map_err(|e| match e {
        // A progressive or CMYK JPEG is a real picture we cannot show yet: say "unsupported".
        image::DecodeError::Jpeg(crate::format::jpeg::JpegError::Unsupported(_)) => {
            ImgFail::Unsupported
        }
        _ => ImgFail::Failed,
    })?;
    let (ow, oh) = (img.width(), img.height());
    let fit_w = fit_w.clamp(16, 4096);
    let mut img = if ow > fit_w {
        img.fit(fit_w, 8192, false, Filter::Auto)
            .map_err(|_| ImgFail::Failed)?
    } else {
        img
    };
    img.flatten(PAGE_BG);
    Ok(Loaded {
        orig_w: ow,
        orig_h: oh,
        img,
    })
}

/// An SVG picture, drawn at its own size or the column's width, whichever is smaller.
fn decode_svg(body: &[u8], fit_w: usize) -> Result<Loaded, ImgFail> {
    let mut img = svg::render(body, fit_w.clamp(16, 4096)).map_err(|e| match e {
        svg::SvgError::NotSvg => ImgFail::Unsupported,
        svg::SvgError::BadSize => ImgFail::TooBig,
        svg::SvgError::OutOfMemory => ImgFail::Failed,
    })?;
    img.flatten(PAGE_BG);
    let (ow, oh) = svg::dims(body).unwrap_or((img.width(), img.height()));
    Ok(Loaded {
        orig_w: ow,
        orig_h: oh,
        img,
    })
}

/// Decode a `data:image/...;base64,` URI (small inline pictures).
pub fn decode_data_uri(uri: &str, fit_w: usize) -> Result<Loaded, ImgFail> {
    if uri.len() > MAX_DATA_URI_LEN {
        return Err(ImgFail::TooBig);
    }
    match base64::decode_data_image(uri, MAX_DATA_IMAGE_BYTES) {
        Ok(bytes) => decode_for_page(&bytes, fit_w),
        Err(base64::DataImageError::Base64(base64::Base64Error::TooLarge)) => Err(ImgFail::TooBig),
        Err(base64::DataImageError::Unsupported) => Err(ImgFail::Unsupported),
        Err(_) => Err(ImgFail::Failed),
    }
}

/// FNV-1a 64-bit, for naming `data:` images without keeping the URI as a key.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The cache key of an image `src` found on the page at `base`: the absolute
/// URL, or `data:#<hash>-<len>` for an inline image. `None` when the address
/// cannot be fetched (not http(s), an https page asking for http: mixed
/// content is refused, malformed).
pub fn image_key(base: &[u8], src: &str) -> Option<String> {
    let src = src.trim();
    if src.is_empty() {
        return None;
    }
    if src.len() >= 5 && src.as_bytes()[..5].eq_ignore_ascii_case(b"data:") {
        let d = base64::parse_data_uri(src)?;
        let _ = d;
        return Some(alloc::format!(
            "data:#{:016x}-{}",
            fnv1a(src.as_bytes()),
            src.len()
        ));
    }
    let b = crate::browsing::browser::parse_url(base)?;
    let abs = crate::browsing::redirect::resolve_redirect(&b, src.as_bytes()).ok()?;
    String::from_utf8(abs).ok()
}

struct Entry {
    key: String,
    slot: Slot,
    used: Cell<u64>,
    /// The `data:` URI to decode (inline images only), dropped once decoded.
    data: Option<String>,
}

enum Slot {
    Pending,
    Loading,
    Ready { ow: usize, oh: usize, img: Image },
    Bad(ImgState),
}

/// The images of the page on screen plus a few recent ones (LRU), bounded by
/// [`CACHE_BYTES`] and [`MAX_ENTRIES`].
pub struct ImageCache {
    entries: Vec<Entry>,
    /// Keys of the page being shown, in document order (at most [`MAX_PAGE_IMAGES`]).
    page: Vec<String>,
    tick: Cell<u64>,
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageCache {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            page: Vec::new(),
            tick: Cell::new(0),
        }
    }

    fn touch(&self, e: &Entry) {
        self.tick.set(self.tick.get() + 1);
        e.used.set(self.tick.get());
    }

    fn find(&self, key: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.key == key)
    }

    /// Start a new page: nothing is wanted yet (loaded images stay cached).
    pub fn begin_page(&mut self) {
        self.page.clear();
    }

    /// Declare that the current page shows `key` (and, for an inline image, its
    /// `data` URI). Returns `false` when the page already has
    /// [`MAX_PAGE_IMAGES`] images or the cache cannot take another entry.
    pub fn want(&mut self, key: &str, data: Option<&str>) -> bool {
        if self.page.iter().any(|k| k == key) {
            return true;
        }
        if self.page.len() >= MAX_PAGE_IMAGES {
            return false;
        }
        if self.find(key).is_none() {
            if self.entries.len() >= MAX_ENTRIES && !self.evict_one(false) {
                return false;
            }
            self.entries.push(Entry {
                key: key.into(),
                slot: Slot::Pending,
                used: Cell::new(0),
                data: data.map(String::from),
            });
        }
        self.page.push(key.into());
        if let Some(i) = self.find(key) {
            let e = &self.entries[i];
            self.touch(e);
        }
        true
    }

    /// Is `key` one of the (at most 8) images of the current page?
    pub fn on_page(&self, key: &str) -> bool {
        self.page.iter().any(|k| k == key)
    }

    /// What the layout should do with `key`.
    pub fn state(&self, key: &str) -> ImgState {
        match self.find(key) {
            Some(i) => match &self.entries[i].slot {
                Slot::Pending | Slot::Loading => ImgState::Pending,
                Slot::Ready { ow, oh, .. } => {
                    self.touch(&self.entries[i]);
                    ImgState::Ready {
                        w: (*ow).min(65535) as i32,
                        h: (*oh).min(65535) as i32,
                    }
                }
                Slot::Bad(s) => *s,
            },
            None if self.on_page(key) || self.page.len() < MAX_PAGE_IMAGES => ImgState::Pending,
            None => ImgState::TooMany,
        }
    }

    /// An image of this page that still has to be fetched or decoded, marked as
    /// in flight. `None` while one is already loading (one at a time) or when
    /// nothing is left. The second value is the `data:` URI of an inline image.
    pub fn next_pending(&mut self) -> Option<(String, Option<String>)> {
        if self.entries.iter().any(|e| matches!(e.slot, Slot::Loading)) {
            return None;
        }
        for k in self.page.clone() {
            if let Some(i) = self.find(&k)
                && matches!(self.entries[i].slot, Slot::Pending)
            {
                self.entries[i].slot = Slot::Loading;
                return Some((k, self.entries[i].data.clone()));
            }
        }
        None
    }

    /// True while some image of the page is pending or in flight.
    pub fn busy(&self) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(e.slot, Slot::Pending | Slot::Loading) && self.on_page(&e.key))
    }

    /// Record the outcome for `key`.
    pub fn finish(&mut self, key: &str, res: Result<Loaded, ImgFail>) {
        let Some(i) = self.find(key) else {
            return;
        };
        let e = &mut self.entries[i];
        e.data = None;
        e.slot = match res {
            Ok(l) => Slot::Ready {
                ow: l.orig_w,
                oh: l.orig_h,
                img: l.img,
            },
            Err(f) => Slot::Bad(f.state()),
        };
        self.tick.set(self.tick.get() + 1);
        self.entries[i].used.set(self.tick.get());
        self.evict_to_budget();
    }

    /// The pixels of `key`, if loaded.
    pub fn image(&self, key: &str) -> Option<&Image> {
        let i = self.find(key)?;
        match &self.entries[i].slot {
            Slot::Ready { img, .. } => {
                self.touch(&self.entries[i]);
                Some(img)
            }
            _ => None,
        }
    }

    /// Scale the stored picture to exactly `w`x`h` (the box the layout gave
    /// it) so painting is a plain copy. Returns whether it changed.
    pub fn fit_to(&mut self, key: &str, w: usize, h: usize) -> bool {
        if w == 0 || h == 0 || w > 4096 || h > 4096 {
            return false;
        }
        let Some(i) = self.find(key) else {
            return false;
        };
        if let Slot::Ready { img, .. } = &mut self.entries[i].slot {
            if img.width() == w && img.height() == h {
                return false;
            }
            if let Ok(n) = img.resize(w, h, Filter::Auto) {
                *img = n;
                self.evict_to_budget();
                return true;
            }
        }
        false
    }

    /// Bytes of pixel memory held.
    pub fn bytes(&self) -> usize {
        self.entries
            .iter()
            .map(|e| match &e.slot {
                Slot::Ready { img, .. } => img.width() * img.height() * 4,
                _ => 0,
            })
            .sum()
    }

    /// Number of entries (any state).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when the cache holds nothing.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drop everything (a navigation that should not keep pictures around).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.page.clear();
    }

    /// Put an in-flight image back to pending (its answer will not come).
    pub fn requeue_loading(&mut self) {
        for e in &mut self.entries {
            if matches!(e.slot, Slot::Loading) {
                e.slot = Slot::Pending;
            }
        }
    }

    /// Remove the least recently used entry that is not loading. With
    /// `allow_page` false the page's own entries are spared. Returns whether
    /// one was removed.
    fn evict_one(&mut self, allow_page: bool) -> bool {
        let victim = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !matches!(e.slot, Slot::Loading))
            .filter(|(_, e)| allow_page || !self.page.contains(&e.key))
            .min_by_key(|(_, e)| e.used.get())
            .map(|(i, _)| i);
        match victim {
            Some(i) => {
                let on_page = self.page.iter().any(|k| *k == self.entries[i].key);
                if on_page {
                    // Still on the page: keep a stub so it does not download again.
                    self.entries[i].slot = Slot::Bad(ImgState::TooBig);
                } else {
                    self.entries.remove(i);
                }
                true
            }
            None => false,
        }
    }

    fn evict_to_budget(&mut self) {
        while self.bytes() > CACHE_BYTES {
            // Prefer entries of other pages; then the oldest picture of this one.
            let victim = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| matches!(e.slot, Slot::Ready { .. }))
                .min_by_key(|(_, e)| (self.page.contains(&e.key), e.used.get()))
                .map(|(i, _)| i);
            let Some(i) = victim else { break };
            if self.page.iter().any(|k| *k == self.entries[i].key) {
                self.entries[i].slot = Slot::Bad(ImgState::TooBig);
            } else {
                self.entries.remove(i);
            }
        }
    }
}

/// The layout's view of the cache for one page: resolves `src` against the
/// page URL, then asks the cache.
pub struct PageImages<'a> {
    pub cache: &'a ImageCache,
    /// Absolute URL of the page (the base for relative `src`).
    pub base: &'a [u8],
}

impl ImageLookup for PageImages<'_> {
    fn lookup(&self, src: &str) -> ImgState {
        match image_key(self.base, src) {
            Some(k) => self.cache.state(&k),
            None => ImgState::Failed,
        }
    }
}

#[cfg(test)]
mod tests;
