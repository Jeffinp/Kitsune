//! The scripts of the page on screen: inline ones are ready at once, external ones are fetched one at a
//! time (like the style sheets) and the whole set is handed to the script process, in document order,
//! when none is still on its way.

use super::*;

/// Most scripts run for one page (inline and external together).
pub const MAX_PAGE_SCRIPTS: usize = 48;
/// Most bytes of script text for one page. Big players and apps ship bundles of megabytes (YouTube's
/// embedded player: 2.5 MB at load); the text sits in the kernel heap until it is handed to the
/// script process, one message per script ([`script_message`]), as fast as the process takes them.
pub const MAX_SCRIPT_TOTAL: usize = 10 * 1024 * 1024;
/// Most bytes of one script: its message (a short header and the source) must fit in one page
/// message (4 MiB, `kitsune_userabi::MAX_PAGE_MSG`).
pub const MAX_ONE_SCRIPT: usize = 4 * 1024 * 1024 - 4096;

/// A script handed to the page: its name (the address, or `inline-N`), its source, and its element's
/// place among the document's `<script>` elements ([`crate::browsing::web::ScriptRef::elem`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageScript {
    pub name: String,
    pub code: String,
    pub elem: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Pending,
    Loading,
    Ready,
    Failed,
}

struct Entry {
    /// The address (external) or `inline-N`.
    name: String,
    state: State,
    code: String,
    elem: usize,
}

/// The scripts of one page.
#[derive(Default)]
pub struct ScriptSet {
    entries: Vec<Entry>,
    bytes: usize,
    started: bool,
    /// The page has `<video>`/`<audio>`: its players live in the script process, so the process
    /// starts even without scripts.
    media: bool,
}

impl ScriptSet {
    /// A new page at `base`: forget the old scripts and take the ones `doc` has.
    pub fn begin(&mut self, doc: &Doc, base: &[u8]) {
        self.entries.clear();
        self.bytes = 0;
        self.started = false;
        self.media = doc.has_media();
        let mut inline = 0;
        for s in doc.scripts().iter().take(MAX_PAGE_SCRIPTS) {
            match &s.src {
                Some(src) => {
                    let key = image_key(base, src).filter(|k| !k.starts_with("data:"));
                    let (name, state) = match key {
                        Some(k) => (k, State::Pending),
                        None => (src.clone(), State::Failed),
                    };
                    self.entries.push(Entry {
                        name,
                        state,
                        code: String::new(),
                        elem: s.elem,
                    });
                }
                None => {
                    inline += 1;
                    let fits = self.bytes.saturating_add(s.text.len()) <= MAX_SCRIPT_TOTAL
                        && s.text.len() <= MAX_ONE_SCRIPT;
                    if fits {
                        self.bytes += s.text.len();
                    }
                    self.entries.push(Entry {
                        name: format!("inline-{inline}"),
                        state: if fits { State::Ready } else { State::Failed },
                        code: if fits { s.text.clone() } else { String::new() },
                        elem: s.elem,
                    });
                }
            }
        }
    }

    /// Forget everything (a page without scripts, or one the browser made itself).
    pub fn clear(&mut self) {
        *self = ScriptSet::default();
    }

    /// The next script to download (it is marked as loading), in document order.
    pub fn next_pending(&mut self) -> Option<String> {
        if self.started {
            return None;
        }
        let e = self
            .entries
            .iter_mut()
            .find(|e| e.state == State::Pending)?;
        e.state = State::Loading;
        Some(e.name.clone())
    }

    /// Is `key` a script of this page that is being downloaded?
    pub fn is_loading(&self, key: &str) -> bool {
        self.entries
            .iter()
            .any(|e| e.name == key && e.state == State::Loading)
    }

    /// The download of `key` ended with `body` (`None`: it failed).
    pub fn finish(&mut self, key: &str, body: Option<&[u8]>) {
        let room = MAX_SCRIPT_TOTAL.saturating_sub(self.bytes);
        let Some(e) = self
            .entries
            .iter_mut()
            .find(|e| e.name == key && e.state == State::Loading)
        else {
            return;
        };
        match body {
            Some(b) if b.len() <= room && b.len() <= MAX_ONE_SCRIPT => {
                e.code = String::from_utf8_lossy(b).into_owned();
                e.state = State::Ready;
                self.bytes += b.len();
            }
            _ => e.state = State::Failed,
        }
    }

    /// A download is queued or in progress.
    pub fn busy(&self) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(e.state, State::Pending | State::Loading))
    }

    /// Every script is in (or failed), at least one can run (or the page has media elements, whose
    /// players need the process), and they were not handed over yet.
    pub fn ready(&self) -> bool {
        !self.started
            && !self.busy()
            && (self.media || self.entries.iter().any(|e| e.state == State::Ready))
    }

    /// Hand the scripts over: each one that arrived, in document order. Later calls return nothing
    /// (a page's scripts run once).
    pub fn take(&mut self) -> Vec<PageScript> {
        if self.started {
            return Vec::new();
        }
        self.started = true;
        self.entries
            .iter_mut()
            .filter(|e| e.state == State::Ready)
            .map(|e| PageScript {
                name: e.name.clone(),
                code: core::mem::take(&mut e.code),
                elem: e.elem,
            })
            .collect()
    }

    /// How many scripts the page has (failed ones included).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
