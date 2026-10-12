//! Internal `kitsune://` pages shown by the browser itself.

use super::*;

impl Browser {
    /// True while an `kitsune://` page is on screen.
    pub fn is_internal(&self) -> bool {
        self.internal
    }

    /// Open the internal page `url` (`kitsune://inicio`, `favoritos`, `historico`, `sobre`).
    /// Unknown names show the "about" page's list of pages.
    pub(in super::super) fn open_internal(&mut self, url: &[u8]) {
        let text = String::from_utf8_lossy(url).to_ascii_lowercase();
        // An address with the old scheme is redirected: the page is shown as `kitsune://...`.
        let rest = text
            .trim_start_matches("kitsune:")
            .trim_start_matches("osjeff:")
            .trim_start_matches('/');
        let (name, query) = rest.split_once('?').unwrap_or((rest, ""));
        let name = name.trim_end_matches('/');
        if name == "inicio" || name.is_empty() {
            self.internal = false;
            self.go_home();
            return;
        }
        // `kitsune://favoritos?rm=N` removes the N-th favourite, then shows the list.
        let mut shown = alloc::format!("kitsune://{name}");
        let doomed = (name == "favoritos")
            .then(|| {
                query
                    .strip_prefix("rm=")
                    .and_then(|v| v.parse::<usize>().ok())
            })
            .flatten()
            .and_then(|n| self.bookmarks.borrow().all().get(n).cloned());
        if let Some(b) = doomed {
            self.bookmarks.borrow_mut().remove(&b.url);
            shown = String::from("kitsune://favoritos");
        }
        self.set_url(shown.as_bytes());
        let n = self.url_len;
        self.nav[..n].copy_from_slice(&self.url[..n]);
        self.nav_len = n;
        self.internal = true;
        self.internal_ready = true;
        self.security = Security::None;
        self.note = None;
        self.pending = false;
        self.home = false;
        self.loaded();
    }

    /// The HTML of an internal page that was just opened (once), for the kernel to lay out.
    pub fn take_internal(&mut self) -> Option<Vec<u8>> {
        if !core::mem::take(&mut self.internal_ready) {
            return None;
        }
        self.internal_html()
    }

    /// The HTML of the internal page on screen, built again in the language in effect (the
    /// kernel calls it when the language changes); `None` when no internal page is shown.
    pub fn internal_html(&self) -> Option<Vec<u8>> {
        if !self.internal {
            return None;
        }
        self.internal_html_in(i18n::lang())
    }

    /// [`Self::internal_html`] in `lang`.
    pub fn internal_html_in(&self, lang: Lang) -> Option<Vec<u8>> {
        if !self.internal {
            return None;
        }
        let name = String::from_utf8_lossy(self.nav_url()).to_ascii_lowercase();
        let name = name
            .trim_start_matches("kitsune://")
            .trim_start_matches("osjeff://");
        Some(match name {
            "favoritos" => pages::bookmarks_in(lang, &self.bookmarks.borrow().all()),
            "historico" => pages::history_in(lang, &self.history.urls),
            _ => pages::about_in(lang),
        })
    }
}
