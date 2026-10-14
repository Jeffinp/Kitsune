//! Desktop compositor: window manager, app rendering, open/close animations,
//! and a process table surfaced through a Tarefas app.
//!
//! All non-trivial logic (terminal, editor, keymap, window geometry, easing,
//! process table, the dynamic window table) lives in `kitsune_core` and is
//! unit-tested. This module is hardware-facing glue: pixels, animation
//! stepping, dispatch, and one [`instance::App`] state per window.

pub(crate) use crate::fb::{Canvas, Color, Corner, Shadow};
pub(crate) use crate::icons::{self, Icon};
pub(crate) use crate::sched;
pub(crate) use crate::sync::RacyCell;
pub(crate) use crate::theme;
pub(crate) use alloc::string::String;
pub(crate) use alloc::vec::Vec;
pub(crate) use kitsune_core::clipboard::{self, Clipboard};
pub(crate) use kitsune_core::window::{MENUBAR_H, ResizeEdge, TITLE_H, WindowId};
pub(crate) use kitsune_core::winman::{ClickTracker, Switcher, WindowManager, WindowSpec};
pub(crate) use kitsune_core::{Calc, Key, Keymap, ProcKind, ProcState, ProcessTable, Rect, Time};
pub(crate) use kitsune_core::{iconart, widgets as wlogic};

/// Longest gap between the two presses of a double click, in timer ticks
/// (250 Hz): 500 ms.
const DOUBLE_CLICK_TICKS: u64 = 125;

// Offscreen texture of a window while it opens / closes / minimises / zooms: the
// window is drawn once at its resting size, then resampled into the animated
// rectangle every frame (see `draw_animating`). Windows larger than this animate
// without the scale effect.
pub(crate) const TEXTURE_BYTES: usize = 1280 * 720 * 4;
#[repr(C, align(64))]
struct AlignedTexture([u8; TEXTURE_BYTES]);
static TEXTURE: RacyCell<AlignedTexture> = RacyCell::new(AlignedTexture([0; TEXTURE_BYTES]));

/// The rectangle a window and its shadow cover (the damage of a moving window):
/// the ambient layer reaches 20 px sideways, 20 above (shifted down 14) and 34 below.
pub(crate) fn shadow_box(r: Rect) -> Rect {
    Rect::new(r.x - 22, r.y - 8, r.w + 44, r.h + 44)
}

/// Cursor sprite bounding box: the area `kitsune_core::cursor::CursorTrack` restores from the back
/// buffer before every frame. Every pointer sprite is rendered into exactly this box
/// (`kitsune_core::pointer::{W, H}`), so no sprite can outgrow what gets erased.
pub const CURSOR_W: i32 = kitsune_core::pointer::W as i32;
pub const CURSOR_H: i32 = kitsune_core::pointer::H as i32;

/// What changed after a mouse packet, so the caller can pick the cheap
/// cursor-only repaint vs a full scene recompose.
pub struct MouseResult {
    pub scene_dirty: bool,
    pub cursor_moved: bool,
}

pub struct Desktop {
    sw: i32,
    // One-time snapshot of the two IDE disks (boot + filesystem) for the Settings disk panel.
    disks: [Option<crate::ata::DiskInfo>; 2],
    sh: i32,
    clipboard: Clipboard,
    /// Paths set aside by the file managers' Copy / Cut (shared by all windows).
    pathclip: kitsune_core::fileman::PathClip,
    keymap: Keymap,
    procs: ProcessTable,
    /// Sampled system history for the resource monitor.
    sysmon: SysMon,
    /// The wallpaper or accent changed: the compositor must repaint the cached background.
    bg_dirty: bool,
    /// The toast overlay (see `shell/toasts.rs`).
    toasts: kitsune_core::notify::Toasts,
    /// Log sequence number up to which WARN+ records already became toasts.
    toast_seen: u32,
    /// A toast appeared or was dismissed since the compositor last repainted them.
    toast_dirty: bool,
    /// Network identity the boot obtained (`nic present`, config), for the settings page.
    /// The dynamic window table; every window owns an app instance.
    wm: WindowManager<Inst>,
    drag: Option<Drag>,
    /// Menus, popovers, sheets, the Apps and Busca overlays and the app bar.
    shell: shell::Shell,
    /// Per-window focus transitions (title bar and shadow cross-fade).
    focus_mix: core::cell::RefCell<Vec<windows::chrome::FocusMix>>,
    /// Local date `(year, month, day)` and weekday (0 = Sunday), refreshed each second.
    today: core::cell::Cell<(i32, u8, u8)>,
    weekday: core::cell::Cell<u8>,
    /// The Alt+Tab switcher while Alt is held.
    switcher: Option<Switcher>,
    /// Window under the cursor.
    hover: Option<WindowId>,
    /// The title-bar button under the pointer (hover fill).
    title_hover: Option<(WindowId, kitsune_core::window::TitleBtn)>,
    /// Which window (and size) the offscreen texture currently holds.
    tex_key: core::cell::Cell<Option<(WindowId, i32, i32)>>,
    clicks: ClickTracker,
    /// Set by operations that change pixels outside the focused window (maximize,
    /// restore, ...); makes the next steady frame upload the whole screen.
    force_full: bool,
    /// A browser window whose client area is the only thing that changed since the last frame
    /// (a scroll, a hover, a keystroke in its bar): the compositor repaints just that area.
    client_dirty: Option<WindowId>,
    /// Region to upload on the next steady frame besides the focused window.
    extra_dirty: Rect,
    cursor_x: i32,
    cursor_y: i32,
    prev_left: bool,
    prev_right: bool,
    /// Installed WASM apps (the Apps overlay lists them after the system apps).
    apps: Vec<AppEntry>,
    /// The WASM window that owns the pressed mouse button.
    wasm_grab: Option<WindowId>,
    /// `netd::now_ms` of the last key or pointer event (the screen locks after a while without).
    last_input_ms: u64,
    /// Last seen generation of app-side clipboard writes.
    clip_gen: u64,
    /// `vfs::generation()` the file managers last loaded, and the tick of that check.
    fs_gen: u32,
    fs_gen_tick: u64,
    /// Reference mode (Ctrl+Alt+R): every frame recomposes the whole scene from scratch, which is
    /// the ground truth the oracle scenarios compare the incremental screen with.
    reference: bool,
    /// Verify mode (Ctrl+Alt+V): every composed frame is compared with a full redraw.
    verify: bool,
}

impl Desktop {
    pub fn new(sw: i32, sh: i32) -> Self {
        // At boot only the system processes and the open terminal exist.
        // Apps spawn a fresh process when opened and are removed when closed.
        let mut procs = ProcessTable::new();
        procs.spawn(b"kernel", ProcKind::System, ProcState::Running);
        procs.spawn(b"compositor", ProcKind::System, ProcState::Running);

        let mut desk = Self {
            disks: [
                crate::ata::identify(0x1F0, 0x3F6, false),
                crate::ata::identify(0x170, 0x376, false),
            ],
            sw,
            sh,
            clipboard: Clipboard::new(),
            pathclip: kitsune_core::fileman::PathClip::new(),
            keymap: Keymap::new(),
            procs,
            sysmon: SysMon::new(),
            bg_dirty: false,
            toasts: kitsune_core::notify::Toasts::new(),
            toast_seen: crate::klog::seq(),
            toast_dirty: false,
            wm: WindowManager::new(kitsune_core::winman::DEFAULT_MAX_WINDOWS),
            drag: None,
            shell: shell::Shell::new(),
            focus_mix: core::cell::RefCell::new(Vec::new()),
            today: core::cell::Cell::new((2026, 1, 1)),
            weekday: core::cell::Cell::new(4),
            switcher: None,
            hover: None,
            title_hover: None,
            tex_key: core::cell::Cell::new(None),
            clicks: ClickTracker::new(DOUBLE_CLICK_TICKS),
            force_full: false,
            client_dirty: None,
            extra_dirty: Rect::new(0, 0, 0, 0),
            cursor_x: sw / 2,
            cursor_y: sh / 2,
            prev_left: false,
            prev_right: false,
            apps: Vec::new(),
            wasm_grab: None,
            last_input_ms: crate::netd::now_ms(),
            clip_gen: crate::wasm::clip_generation(),
            fs_gen: vfs::generation(),
            fs_gen_tick: 0,
            reference: false,
            verify: false,
        };
        // Install the bundled apps into /apps (first boot) and build the launcher catalog.
        crate::trace::mark("  Desktop::new: struct built");
        desk.init_apps();
        crate::trace::mark("  Desktop::new: apps installed + catalog");
        // The sign-in screen when accounts need one (several users, or a password set); else the
        // terminal is open (and focused) at boot.
        if services::accounts::login_required() && !services::accounts::signed_in() {
            desk.open_login(false);
        } else {
            desk.open_new(Kind::Terminal);
        }
        desk
    }

    /// Everything that decides the cursor's pixels: the top-left of the sprite box and which
    /// sprite (arrow, hand, I-beam). The compositor repaints the sprite whenever this differs
    /// from what it painted.
    pub fn pointer(&self) -> kitsune_core::cursor::Pointer {
        let (x, y) = self.cursor();
        kitsune_core::cursor::Pointer {
            x,
            y,
            shape: self.cursor_shape() as u8,
        }
    }

    /// Top-left of the pointer sprite box (the pointer position minus the hotspot).
    pub fn cursor(&self) -> (i32, i32) {
        let (hx, hy) = kitsune_core::pointer::hotspot(self.cursor_shape());
        (self.cursor_x - hx, self.cursor_y - hy)
    }
}

mod apps;
pub(crate) use apps::browser::{loop_start, note_present, schedule_next, video_active};
mod compositor;
mod frame;
mod input;
mod kit;
mod services;
mod shell;
mod windows;

pub(crate) use apps::ajustes::SettingsState;
pub(crate) use apps::browser::{
    BrowserHover, BrowserState, PageCmd, PageMenu, StripEntry, TabData,
};
pub(crate) use apps::editor::EditorState;
pub(crate) use apps::files::*;
pub(crate) use apps::registro::LogState;
pub use apps::tarefas::SysInputs;
pub(crate) use apps::tarefas::{SysMon, TarefasState};
pub(crate) use apps::terminal::TermState;
pub(crate) use apps::viewer::*;
pub(crate) use apps::wasm::*;
pub use compositor::{Compositor, FrameIn, Screen};
pub(crate) use input::Special;
pub(crate) use kit::widgets::*;
use kit::{appui, ui};
pub use services::shellhost::{worker as shell_worker, worker2 as shell_worker2};

/// The signed-in user's home folder (`/home` before anyone signs in).
pub fn accounts_home() -> alloc::vec::Vec<u8> {
    services::accounts::home()
}

/// Load the account database and sign in the lone no-password user (once, after the storage).
pub fn init_accounts() {
    services::accounts::init();
}
pub(crate) use services::sysstore::*;
pub(crate) use services::vfs;
pub(crate) use windows::drag::{Drag, DragMode};
pub(crate) use windows::instance::*;
