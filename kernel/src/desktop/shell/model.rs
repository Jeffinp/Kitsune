//! The shell's data: commands, menu entries, overlays, the confirmation dialog and the animation helpers.

use crate::desktop::kit::glass::BackdropSlot;
use crate::desktop::*;
use core::cell::Cell;
use kitsune_core::anim::{Tween, curves};
use kitsune_core::chrome::{MenuGeom, MenuRow};
use kitsune_core::raster::Surface;
use kitsune_core::snap::SnapZone;

/// Everything a menu entry or a button can ask the desktop to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Cmd {
    /// A separator line (menus only).
    Sep,
    About,
    Settings,
    Reboot,
    Shutdown,
    NewWindow,
    CloseWindow,
    Minimize,
    Zoom,
    Copy,
    Cut,
    Paste,
    SelectAll,
    Undo,
    Redo,
    OpenFile,
    SaveFile,
    ShowApps,
    ShowSearch,
    Gallery,
    Activate(WindowId),
    Launch(Kind),
    BrowserZoomIn,
    BrowserZoomOut,
    BrowserZoomReset,
    /// Tile the focused window to a half or a quarter of the work area.
    Snap(SnapZone),
    /// Open one more window of an app (taskbar menu).
    NewOf(Kind),
    /// Move the focused window to workspace `n` (0-based) and go there.
    MoveToWorkspace(u8),
    /// Pin an app to the taskbar / take it off.
    Pin(Kind),
    Unpin(Kind),
    /// Minimise every window, or bring back the ones that command hid.
    ShowDesktop,
    /// Close every window of an app (app-bar menu).
    QuitOf(Kind),
    /// A command of the file manager (its context and sort menus, the View menu).
    Files(kitsune_core::fileman::Cmd),
    /// Lock the screen: the session and its windows stay, a password brings them back.
    Lock,
    /// Close everything and go back to the sign-in screen (to change user).
    SignOut,
}

/// One row of a menu.
#[derive(Clone)]
pub(crate) struct Entry {
    pub label: String,
    pub shortcut: &'static str,
    pub cmd: Cmd,
    pub enabled: bool,
    pub checked: bool,
}

impl Entry {
    pub(crate) fn item(label: &str, shortcut: &'static str, cmd: Cmd) -> Entry {
        Entry {
            label: String::from(label),
            shortcut,
            cmd,
            enabled: true,
            checked: false,
        }
    }

    pub(crate) fn sep() -> Entry {
        Entry::item("", "", Cmd::Sep)
    }
}

/// An item of the top panel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PanelItem {
    /// The Kitsune mark and "Apps": opens the launcher.
    Apps,
    /// Busca.
    Search,
    /// The workspace dots: click one to go there.
    Workspaces,
    /// The date and time: the calendar and notification centre.
    Clock,
    /// The status pill: Quick Settings.
    Tray,
}

/// Where an open menu came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MenuOrigin {
    /// The menu button in a window's title bar.
    Window(WindowId),
    /// A right-click menu (desktop or an app-bar icon).
    Context,
}

/// An open menu (dropdown or context menu), fading in.
pub(crate) struct OpenMenu {
    pub origin: MenuOrigin,
    pub entries: Vec<Entry>,
    pub rows: Vec<MenuRow>,
    pub geom: MenuGeom,
    pub hover: Option<usize>,
    pub t: Tween,
    pub closing: bool,
    pub glass: BackdropSlot,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PopKind {
    /// Quick Settings, under the status pill.
    Quick,
    /// The calendar and notification centre, under the clock.
    Centre,
}

/// A popover under a panel item.
pub(crate) struct Popover {
    pub kind: PopKind,
    pub rect: Rect,
    pub t: Tween,
    pub closing: bool,
    pub glass: BackdropSlot,
    /// Calendar: months shown relative to the current one.
    pub month_off: i32,
}

/// A confirmation sheet (restart / shut down).
pub(crate) struct Dialog {
    pub title: String,
    pub body: String,
    pub ok: String,
    pub cmd: Cmd,
    pub t: Tween,
    pub closing: bool,
    /// 0 = cancel, 1 = confirm (keyboard focus).
    pub focus: usize,
}

/// The sign-in and lock screen: a full-screen layer over everything, taking every key and click.
pub(crate) struct LoginView {
    /// The accounts to choose from (just the signed-in one when the screen is locked).
    pub users: Vec<crate::desktop::services::accounts::UserRow>,
    pub sel: usize,
    /// The password typed so far (printable ASCII, as the keyboard layer delivers it).
    pub pw: kitsune_core::secret::Secret,
    /// A line under the field and whether it is an error.
    pub msg: Option<(String, bool)>,
    /// A locked screen keeps the session and its windows; signing in starts one.
    pub locked: bool,
    pub t: Tween,
    pub closing: bool,
}

/// What a tile of the Apps overlay opens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Target {
    Kind(Kind),
    Wasm(usize),
}

pub(crate) struct Tile {
    pub label: String,
    pub target: Target,
    pub icon: Surface,
    /// Its category in the rail and the key *Recentes* remembers it by.
    pub cat: kitsune_core::launcher::Category,
    pub key: String,
}

/// The Apps overlay: every app in a grid with a search field.
pub(crate) struct AppsView {
    pub tiles: Vec<Tile>,
    /// The rail's selection (index into `launcher::CATEGORIES`) and the row under the pointer.
    pub cat: usize,
    pub rail_hover: Option<usize>,
    pub query: String,
    /// Indices into `tiles` matching `query`.
    pub shown: Vec<usize>,
    pub scroll: usize,
    pub hover: Option<usize>,
    pub t: Tween,
    pub closing: bool,
    pub backdrop: BackdropSlot,
}

/// What a Busca result does.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum HitKind {
    App(Target),
    /// An alias that opens Tarefas on a tab ("monitor", "memória", ...).
    Tab(u8),
    File(Vec<u8>),
    Calc,
}

#[derive(Clone)]
pub(crate) struct SearchHit {
    pub title: String,
    pub sub: String,
    pub kind: HitKind,
}

/// The Busca overlay: one field; results are apps, files and calculator answers.
pub(crate) struct SearchView {
    pub query: String,
    pub hits: Vec<SearchHit>,
    pub sel: usize,
    /// Files known to the index: `(path, name)`, built when it opened.
    pub files: Vec<(Vec<u8>, String)>,
    pub t: Tween,
    pub closing: bool,
    pub glass: BackdropSlot,
}

/// The outline previewing where a dragged window snaps: it travels from the window's rectangle
/// to the zone's while it fades in.
pub(crate) struct SnapPreview {
    pub zone: SnapZone,
    pub from: Rect,
    pub to: Rect,
    pub t: Tween,
}

/// All shell state of the desktop.
pub(crate) struct Shell {
    /// The snap preview while a window is dragged to an edge.
    pub snap: Option<SnapPreview>,
    pub menu: Option<OpenMenu>,
    pub pop: Option<Popover>,
    pub dialog: Option<Dialog>,
    pub login: Option<LoginView>,
    pub apps: Option<AppsView>,
    pub search: Option<SearchView>,
    pub task: super::taskbar::TaskbarState,
    /// Panel item under the pointer (highlight).
    pub panel_hover: Option<PanelItem>,
    /// The notification centre's history (oldest first), the log cursor that feeds it and how
    /// many entries arrived since the centre was last opened.
    pub notifs: Vec<super::panel::Notif>,
    pub notif_seen: u32,
    pub notif_unread: usize,
    /// The apps launched most recently (the launcher's *Recentes* row).
    pub recents: kitsune_core::launcher::Recents,
    /// The performance HUD (Ctrl+Alt+H).
    pub hud: bool,
    /// Region of the Apps overlay that changed since it was last painted.
    pub dirty: Cell<Rect>,
    /// Quick Settings and the notification centre: their switches animate here.
    pub knobs: [Tween; 3],
    /// Last time of day (seconds) the appearance was resolved for (Auto).
    pub last_hour: u8,
    /// Blurred backdrop of the Alt+Tab panel, captured when it first draws.
    pub switcher_glass: BackdropSlot,
}

impl Shell {
    pub(crate) fn new() -> Shell {
        Shell {
            snap: None,
            menu: None,
            pop: None,
            dialog: None,
            login: None,
            apps: None,
            search: None,
            task: super::taskbar::TaskbarState::new(),
            panel_hover: None,
            notifs: Vec::new(),
            notif_seen: crate::klog::seq(),
            notif_unread: 0,
            recents: kitsune_core::launcher::Recents::new(),
            hud: false,
            dirty: Cell::new(Rect::new(0, 0, 0, 0)),
            knobs: [Tween::at(0.0); 3],
            last_hour: 255,
            switcher_glass: Default::default(),
        }
    }
}

/// Fade-in seconds of menus and popovers, and of the full-screen overlays.
pub(crate) const MENU_FADE: f32 = 0.14;
pub(crate) const OVERLAY_FADE: f32 = 0.22;

/// A tween starting a fade from nothing.
pub(crate) fn fade_in(secs: f32) -> Tween {
    let mut t = Tween::at(0.0);
    t.retarget(1.0, secs, curves::ENTER);
    t
}

pub(crate) fn fade_out(t: &mut Tween, secs: f32) {
    t.retarget(0.0, secs, curves::EXIT);
}

/// Opacity 0..=256 of a fade tween.
pub(crate) fn level(t: &Tween) -> u32 {
    (t.value().clamp(0.0, 1.0) * 256.0) as u32
}
