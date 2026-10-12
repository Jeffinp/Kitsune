//! State of a browser window: tabs, the strip, the page menu and the bookmark store.

use crate::desktop::*;
use alloc::boxed::Box;

/// What the pointer is over in a browser window (hover looks follow it).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum BrowserHover {
    #[default]
    None,
    Back,
    Forward,
    Reload,
    NewTab,
    Star,
    Shield,
    Bar,
    /// A tab of the strip (index into the tabs, not the strip).
    Tab(usize),
    TabClose(usize),
    Suggestion(usize),
    /// A row of the page's context menu.
    MenuRow(usize),
    /// A tile or row of the start page.
    Tile(usize),
    Recent(usize),
    /// A button of the find bar: previous, next, close.
    FindPrev,
    FindNext,
    FindClose,
    /// The error page's buttons.
    Retry,
    Proceed,
}

/// One tab of a browser window: its own history, page, scroll, forms and find state. Only
/// the active tab is laid out and painted; the others keep their parsed document.
pub(crate) struct TabData {
    /// Never reused within a window: a result from the network finds its tab by this.
    pub id: u32,
    pub browser: kitsune_core::Browser,
    /// The parsed document (kept so the page can be laid out again without parsing).
    pub doc: Option<kitsune_core::web::Doc>,
    pub page: Option<kitsune_core::web::Page>,
    /// The scroll offset being painted (the spring below eases it to its target).
    pub scroll: i32,
    pub scroll_spring: kitsune_core::anim::Spring,
    pub scroll_bar: kitsune_core::widgets::ScrollbarFade,
    /// Viewport width `page` was laid out for.
    pub layout_w: i32,
    /// Cache key of each `page.images` entry (`None`: not fetchable).
    pub img_keys: Vec<Option<String>>,
    /// The external style sheets of the page (which are asked for, which arrived).
    pub sheets: kitsune_core::web::sheets::SheetSet,
    /// Page zoom in percent.
    pub zoom: u16,
    /// What the user typed into the page's form controls.
    pub forms: kitsune_core::web::form::FormState,
    /// Ctrl+F bar and matches.
    pub find: kitsune_core::web::find::FindBar,
    /// Start of a mouse selection (page coordinates) and the selected text.
    pub sel_anchor: Option<(i32, i32)>,
    pub sel: Option<kitsune_core::web::textops::Selection>,
    /// The link under the pointer (index into the page's links), for the hover underline.
    pub hover_link: Option<usize>,
    /// The certificate summary of the page on screen (https only).
    pub cert: Option<kitsune_core::browser::CertInfo>,
    /// The progress bar under the omnibox.
    pub load: kitsune_core::browser::motion::LoadBar,
    /// Fill of the favourite star, 0 (outline) to 1 (filled).
    pub star: kitsune_core::anim::Tween,
    /// The load in flight was stopped: its result is dropped when it arrives.
    pub cancelled: bool,
    /// `perf-trace`: when the page was handed over, until its first paint is reported.
    pub trace_t0: core::cell::Cell<u64>,
}

/// A tab as the strip shows it: it grows when it opens and shrinks when it closes (a closed
/// tab stays as a "ghost" until its animation ends).
pub(crate) struct StripEntry {
    /// The tab's id, or `None` for a ghost.
    pub id: Option<u32>,
    pub weight: kitsune_core::anim::Tween,
    /// What a ghost still shows.
    pub title: String,
    pub badge: char,
}

/// Browser window state: the tabs (each with its own page), what they share (the image
/// cache, the fetcher's single slot) and the chrome's own transient state. `Deref`s to the
/// active tab so the code that works on "the page on screen" just says `b.page`.
pub(crate) struct BrowserState {
    pub tabs: kitsune_core::browser::tabs::TabList<TabData>,
    /// Pictures of the page being shown (and a few recent ones), shared by the tabs.
    pub images: kitsune_core::web::imgcache::ImageCache,
    /// The picture the fetcher is working on, and the tab that asked for it.
    pub img_inflight: Option<(u32, String)>,
    /// The style sheet the fetcher is working on, and the tab that asked for it.
    pub sheet_inflight: Option<(u32, String)>,
    /// The tab whose page request the fetcher has.
    pub req_tab: Option<u32>,
    pub next_id: u32,
    /// The strip in visual order (ghosts included).
    pub strip: Vec<StripEntry>,
    /// Height of the tab strip while it appears and disappears.
    pub strip_h: kitsune_core::anim::Tween,
    pub hover: BrowserHover,
    /// The security popover is open.
    pub popover: bool,
    /// The page's context menu: where it opened, what it can do.
    pub ctx: Option<PageMenu>,
    /// The line over the bottom of the page ("Favorito adicionado") and its fade.
    pub notice: Option<String>,
    pub notice_flash: kitsune_core::browser::motion::Flash,
    /// The zoom pill's fade.
    pub zoom_flash: kitsune_core::browser::motion::Flash,
    /// The frame where the omnibox suggestions, the popover, the find bar and the menu take
    /// their blurred backdrop from.
    pub glass: [crate::desktop::kit::glass::BackdropSlot; 4],
    /// The page area as last painted (see `paint/`).
    pub cache: core::cell::RefCell<crate::desktop::apps::browser::PaintCache>,
    /// Bumped by everything that changes how the page area looks, so the cache knows.
    pub rev: u64,
}

/// The page's context menu.
pub(crate) struct PageMenu {
    pub x: i32,
    pub y: i32,
    /// The link under the pointer when it opened.
    pub link: Option<String>,
    /// Page coordinates of the click (to ask what was under it).
    pub items: Vec<(PageCmd, &'static str, bool)>,
}

/// What a context menu row does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PageCmd {
    Copy,
    OpenLink,
    CopyLink,
    Back,
    Reload,
    Bookmark,
}

impl core::ops::Deref for BrowserState {
    type Target = TabData;
    fn deref(&self) -> &TabData {
        self.tabs.active()
    }
}

impl core::ops::DerefMut for BrowserState {
    fn deref_mut(&mut self) -> &mut TabData {
        self.tabs.active_mut()
    }
}

impl TabData {
    pub(crate) fn new(id: u32, browser: kitsune_core::Browser) -> TabData {
        TabData {
            id,
            browser,
            doc: None,
            page: None,
            scroll: 0,
            scroll_spring: kitsune_core::anim::Spring::pixels(0.0, 260.0, 30.0),
            scroll_bar: kitsune_core::widgets::ScrollbarFade::new(),
            layout_w: 0,
            img_keys: Vec::new(),
            sheets: kitsune_core::web::sheets::SheetSet::default(),
            zoom: 100,
            forms: kitsune_core::web::form::FormState::default(),
            find: kitsune_core::web::find::FindBar::new(),
            sel_anchor: None,
            sel: None,
            hover_link: None,
            cert: None,
            load: kitsune_core::browser::motion::LoadBar::new(),
            star: kitsune_core::anim::Tween::at(0.0),
            cancelled: false,
            trace_t0: core::cell::Cell::new(0),
        }
    }
}

impl BrowserState {
    pub(crate) fn new() -> BrowserState {
        let first = TabData::new(1, kitsune_core::Browser::with_store(new_bookmark_store()));
        BrowserState {
            tabs: kitsune_core::browser::tabs::TabList::new(first),
            images: kitsune_core::web::imgcache::ImageCache::new(),
            img_inflight: None,
            sheet_inflight: None,
            req_tab: None,
            next_id: 2,
            strip: alloc::vec![StripEntry {
                id: Some(1),
                weight: kitsune_core::anim::Tween::at(1.0),
                title: String::new(),
                badge: ' ',
            }],
            strip_h: kitsune_core::anim::Tween::at(0.0),
            hover: BrowserHover::None,
            popover: false,
            ctx: None,
            notice: None,
            notice_flash: kitsune_core::browser::motion::Flash::new(),
            zoom_flash: kitsune_core::browser::motion::Flash::new(),
            glass: Default::default(),
            cache: core::cell::RefCell::new(Default::default()),
            rev: 1,
        }
    }
}

/// The single place that decides where the browser's favourites live: the file
/// `/home/.bookmarks` on the desktop volume (written after every change; a missing or
/// damaged file starts an empty list).
pub(crate) fn new_bookmark_store() -> Box<dyn kitsune_core::browser::BookmarkStore> {
    const PATH: &[u8] = b"/home/.bookmarks";
    let text = crate::desktop::services::vfs::read_file(PATH).unwrap_or_default();
    Box::new(kitsune_core::browser::SavedBookmarks::load(&text, |t| {
        if !crate::desktop::services::vfs::exists(b"/home") {
            let _ = crate::desktop::services::vfs::mkdir(b"/home");
        }
        if crate::desktop::services::vfs::write_file(PATH, t).is_err() {
            crate::notify::notify_key(
                crate::klog::Level::Warn,
                kitsune_core::tk!("notify.bookmarks_unsaved"),
                &[],
            );
        }
    }))
}
