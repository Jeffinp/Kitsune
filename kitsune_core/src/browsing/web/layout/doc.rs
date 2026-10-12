//! A parsed document and how it is laid out: `Doc`, `render` and `render_with`.

use super::*;

/// A parsed document: the DOM and the stylesheet, so a page can be laid out
/// again (resize, zoom, an image arriving) without parsing the HTML again.
pub struct Doc {
    pub(crate) dom: Vec<Node>,
    pub(crate) sheet: Stylesheet,
    pub(crate) title: String,
    /// Rules of the user-agent sheet, and of the external sheets added so far (they sit between
    /// the user-agent rules and the page's own `<style>` rules).
    pub(crate) ua_rules: usize,
    pub(crate) ext_rules: usize,
    pub(crate) quirks: bool,
}

impl Doc {
    /// Parse `html` (tolerant of any input, see [`parse_html`]).
    pub fn parse(html: &[u8]) -> Doc {
        let (mut dom, css) = parse_html(html);
        // Every document has a root element and a body: fragments get them, so `:root` rules, the
        // user-agent margins and the page's own `body { ... }` rules apply to them too.
        let one = |i: u16| crate::browsing::web::dom::Sib {
            index: i,
            count: 1,
            tindex: 1,
            tcount: 1,
        };
        if !has_root(&dom) {
            dom = alloc::vec![Node::Element(Element {
                tag: String::from("html"),
                attrs: Default::default(),
                children: dom,
                ws_before: false,
                sib: one(1),
            })];
        }
        if !has_body(&dom, 0)
            && let Some(Node::Element(root)) = dom
                .iter_mut()
                .find(|n| matches!(n, Node::Element(e) if e.tag == "html"))
        {
            let inner = core::mem::take(&mut root.children);
            root.children = alloc::vec![Node::Element(Element {
                tag: String::from("body"),
                attrs: Default::default(),
                children: inner,
                ws_before: false,
                sib: one(1),
            })];
        }
        let mut sheet = parse_css(UA_CSS);
        for r in &mut sheet.rules {
            r.author = false;
        }
        let ua_rules = sheet.rules.len();
        let own = parse_css(&css);
        let base = sheet.medias.len();
        sheet.medias.extend(own.medias);
        sheet.imports = own.imports;
        sheet.has_pseudo |= own.has_pseudo;
        sheet.rules.extend(own.rules.into_iter().map(|mut r| {
            if r.media != 0 {
                r.media += base as u16;
            }
            r
        }));
        sheet.build_index();
        let title = find_title(&dom, 0)
            .map(|t| fold_display(&t))
            .unwrap_or_default();
        Doc {
            dom,
            sheet,
            title,
            ua_rules,
            ext_rules: 0,
            quirks: !html[..html.len().min(1024)]
                .to_ascii_lowercase()
                .windows(9)
                .any(|w| w == b"<!doctype"),
        }
    }

    /// Addresses of the style sheets the document asks for, in order: `<link rel=stylesheet href>`
    /// (screen or unconditional ones) and the `@import`s of its own `<style>` text.
    pub fn linked_sheets(&self) -> Vec<String> {
        fn walk(nodes: &[Node], depth: usize, out: &mut Vec<String>) {
            if depth > 8 || out.len() >= 32 {
                return;
            }
            for n in nodes {
                let Node::Element(e) = n else { continue };
                if e.tag == "link" {
                    let rel = e.attrs.get("rel").map_or("", |r| r.as_str());
                    let is_sheet = rel
                        .split_ascii_whitespace()
                        .any(|w| w.eq_ignore_ascii_case("stylesheet"))
                        && !rel
                            .split_ascii_whitespace()
                            .any(|w| w.eq_ignore_ascii_case("alternate"));
                    let media_ok = e.attrs.get("media").is_none_or(|m| {
                        crate::browsing::web::css::Media::parse(m).is_some_and(|m| {
                            m.matches(
                                crate::browsing::web::css::DEFAULT_W,
                                crate::browsing::web::css::DEFAULT_H,
                            )
                        })
                    });
                    if let (true, true, Some(href)) = (is_sheet, media_ok, e.attrs.get("href")) {
                        let href = crate::browsing::web::dom::decode_attr(href);
                        if !href.trim().is_empty() {
                            out.push(href);
                        }
                    }
                } else {
                    walk(&e.children, depth + 1, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.dom, 0, &mut out);
        // The page's own `@import`s come after its links (they are in a `<style>`).
        out.extend(self.sheet.imports.iter().cloned());
        out
    }

    /// Add the rules of an external style sheet. They count as written before the page's own
    /// `<style>` rules and after the external sheets added earlier. Returns the sheet's `@import`s.
    pub fn add_sheet(&mut self, css: &str) -> Vec<String> {
        let new = parse_css(css);
        let base = self.sheet.medias.len();
        let room = MAX_RULES.saturating_sub(self.sheet.rules.len());
        self.sheet.medias.extend(new.medias);
        self.sheet.has_pseudo |= new.has_pseudo;
        let take = new.rules.len().min(room);
        let at = self.ua_rules + self.ext_rules;
        let rules: Vec<_> = new
            .rules
            .into_iter()
            .take(take)
            .map(|mut r| {
                if r.media != 0 {
                    r.media += base as u16;
                }
                r
            })
            .collect();
        self.ext_rules += rules.len();
        self.sheet.rules.splice(at..at, rules);
        self.sheet.build_index();
        new.imports
    }

    /// The document's `<title>` text (empty if none).
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Lay the document out.
    pub fn layout(&self, opts: &Layout) -> Page {
        let zoom = i32::from(opts.zoom.clamp(MIN_ZOOM, MAX_ZOOM));
        // `vw` and `@media` see the width in CSS pixels (the zoom is not part of it).
        crate::browsing::web::css::set_viewport(
            (i64::from(opts.width) * 100 / i64::from(zoom)) as i32,
            crate::browsing::web::css::DEFAULT_H,
        );
        let root = Computed::root();
        let mut p = Painter {
            cmds: Vec::new(),
            links: Vec::new(),
            hits: Vec::new(),
            images: Vec::new(),
            forms: Vec::new(),
            fields: Vec::new(),
            cur_form: None,
            zoom,
            lookup: opts.images,
            m: opts.metrics,
            sheet: &self.sheet,
            anc: Vec::new(),
            budget: STYLE_BUDGET,
            chars: 0,
            items: Vec::new(),
            spare_items: Vec::new(),
            fm: core::cell::RefCell::new(Vec::new()),
            space: None,
            link: None,
            pending: 0,
            marker: None,
            lists: Vec::new(),
            canvas: None,
            last_box: (None, None),
            floats: Vec::new(),
            quirks: self.quirks,
        };
        let area = Area {
            x: 0,
            w: opts.width.max(40),
        };
        let mut y = flow(&mut p, &self.dom, &root, &root, area, 0);
        y = flush_inline(&mut p, &root, area, y, None);
        y = adv(y, core::mem::take(&mut p.pending));
        // A float may reach below the last line.
        y = y.max(floats_bottom(&p, 0));
        Page {
            cmds: p.cmds,
            height: y.clamp(0, MAX_PAGE_H),
            background: p.canvas.unwrap_or(Rgb(255, 255, 255)),
            links: p.links,
            hits: p.hits,
            images: p.images,
            forms: p.forms,
            fields: p.fields,
        }
    }
}

pub(crate) fn has_root(nodes: &[Node]) -> bool {
    nodes
        .iter()
        .any(|n| matches!(n, Node::Element(e) if e.tag == "html"))
}

pub(crate) fn has_body(nodes: &[Node], depth: usize) -> bool {
    depth <= 3
        && nodes.iter().any(|n| match n {
            Node::Element(e) => e.tag == "body" || has_body(&e.children, depth + 1),
            Node::Text(_) => false,
        })
}

/// The text of the first `<title>` element (depth-limited: the DOM is
/// page-controlled).
pub(crate) fn find_title(nodes: &[Node], depth: usize) -> Option<String> {
    if depth > 8 {
        return None;
    }
    for n in nodes {
        if let Node::Element(e) = n {
            if e.tag == "title" {
                let t = text_content(&e.children);
                let t = t.trim();
                return (!t.is_empty()).then(|| t.into());
            }
            if let Some(t) = find_title(&e.children, depth + 1) {
                return Some(t);
            }
        }
    }
    None
}

/// Render an HTML document to a display list laid out for `viewport_w` pixels
/// with the deterministic [`FixedAdvance`] metrics (no images known, zoom 100%).
/// For tests and fuzzing; the kernel calls [`Doc::layout`] with its own metrics.
pub fn render(html: &[u8], viewport_w: i32) -> Page {
    render_with(html, viewport_w, &FixedAdvance)
}

/// [`render`] with the caller's metrics.
pub fn render_with(html: &[u8], viewport_w: i32, metrics: &dyn TextMetrics) -> Page {
    Doc::parse(html).layout(&Layout {
        width: viewport_w,
        zoom: 100,
        images: &NoImages,
        metrics,
    })
}

// ---- the painter: state of one layout pass ----
