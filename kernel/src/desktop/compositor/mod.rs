//! The compositor: one frame of the desktop, from "something happened" to pixels on screen.
//!
//! ```text
//!   Desktop state ──build_scene──▶ Scene ──Engine::plan──▶ Plan { damage, steps }
//!                                                              │
//!        framebuffer ◀──upload damage── back buffer ◀──paint steps (bottom to top)
//!        (+ toasts, HUD, cursor: framebuffer only)
//! ```
//!
//! There is exactly one rendering path. A drag, a keystroke, a clock tick, an animation, a
//! popover opening and a window leaving the screen all reach the engine as the *same kind of
//! input* (a changed scene, an invalidated rectangle), and the engine answers with the damage
//! and the paint calls: no cached "static layer", no per-case repaint code, no signature to
//! extend. See `docs/design/compositor.md` for the design, the invariants and how to extend it.
//!
//! Files: `layers.rs` describes the desktop as a scene, `paint.rs` paints a layer, `present.rs`
//! owns the buffers and the framebuffer-only passes (toasts, HUD, cursor), `verify.rs` is the
//! debugging check that the incremental screen equals a full redraw.

mod animating;
mod layers;
mod paint;
mod present;
mod verify;

use super::frame::Animation;
use super::*;
use crate::perf::Perf;
use crate::trace;
use kitsune_core::compositor::{Engine, Plan};
use layers::Epochs;
use present::FbPasses;
pub use present::Screen;

/// What happened since the last frame, as far as the compositor cares.
pub struct FrameIn {
    pub time: Time,
    /// Timer ticks now, and whether any elapsed since the previous frame.
    pub tick: u64,
    pub tick_changed: bool,
    /// A handler reported a visible change (a keystroke, a click), or a full repaint was asked.
    pub input: bool,
    /// The desktop asked for a full repaint (maximise, theme change...).
    pub force_full: bool,
    /// Extra region the handlers marked as changed.
    pub extra_dirty: Option<Rect>,
    /// A browser window whose client area is the only thing that changed (a scroll, a hover, a
    /// keystroke in its bar): just that area is repainted, not the frame and shadow around it.
    pub client: Option<WindowId>,
    /// An asynchronous result (a fetched page) changed a window that may not have the focus.
    pub external: bool,
    pub cursor_moved: bool,
    /// The wall clock moved to a new second.
    pub clock_tick: bool,
}

/// Why this frame is rendered (see [`Compositor::frame`]).
#[derive(Clone, Copy)]
struct Cause {
    input: bool,
    hover: bool,
    tick: bool,
    settle: bool,
    clock: bool,
}

/// Result of a frame, for the caller's bookkeeping.
pub struct FrameOut {
    /// The frame did rendering work (timed, and the cursor was repainted).
    pub worked: bool,
}

/// Ticks (4 ms) between two frames of moving animations: at most 125 a second.
const MOTION_TICKS: u64 = 2;
/// Ticks between two frames of an ambient animation (a caret's blink): about 31 a second.
const AMBIENT_TICKS: u64 = 8;

/// The desktop compositor.
pub struct Compositor {
    engine: Engine,
    passes: FbPasses,
    epochs: Epochs,
    /// Windows repainted every frame last time: when one stops, it needs one last repaint.
    dynamic_prev: Vec<WindowId>,
    prev_focused: Option<WindowId>,
    was_anim: bool,
    /// Tick of the last frame that repainted live windows (they glide at 50 fps).
    live_tick: u64,
    /// Tick of the last frame an animation asked for (see `MOTION_TICKS`).
    anim_tick: u64,
    /// Debugging: compare every frame with a full redraw (Ctrl+Alt+V).
    verify: verify::Verifier,
}

impl Compositor {
    pub fn new(width: i32, height: i32) -> Self {
        Self {
            engine: Engine::new(width, height),
            passes: FbPasses::new(),
            epochs: Epochs::default(),
            dynamic_prev: Vec::new(),
            prev_focused: None,
            was_anim: false,
            live_tick: 0,
            anim_tick: 0,
            verify: verify::Verifier::new(),
        }
    }

    /// The wallpaper or accent changed: everything is repainted at the next frame.
    pub fn repaint_everything(&mut self) {
        self.engine.invalidate_all();
    }

    /// Render and present one frame of `desk`: erase the cursor, compose what changed into the
    /// back buffer, upload exactly the damage, redraw the toasts and the HUD, paint the cursor.
    pub fn frame(
        &mut self,
        desk: &mut Desktop,
        scr: &mut Screen,
        perf: &mut Perf,
        i: &FrameIn,
    ) -> FrameOut {
        let anim = desk.animation();
        let any_anim = anim != Animation::None;
        let cursor_moved = i.cursor_moved | self.passes.stale(desk);
        let settle = self.was_anim && !any_anim;
        let hover = cursor_moved && desk.overlay_open();
        // Animations are timed in real time, so frames beyond what the eye or the screen takes
        // only burn CPU: at most one every `MOTION_TICKS` (125 Hz) for moving things and one every
        // `AMBIENT_TICKS` (31 Hz) for a blinking caret. A moved pointer always gets its frame.
        let gap = if anim == Animation::Motion {
            MOTION_TICKS
        } else {
            AMBIENT_TICKS
        };
        let anim_due = any_anim && i.tick_changed && i.tick.wrapping_sub(self.anim_tick) >= gap;
        let tick = (anim_due || (any_anim && cursor_moved)) && {
            self.anim_tick = i.tick;
            true
        };
        let reference = desk.reference_mode() && (any_anim || i.input || i.clock_tick);
        let due = i.input || hover || tick || settle || i.clock_tick || i.client.is_some();
        // (`settle` is the frame after an animation: it needs the cursor passes too.)
        let work = tick || i.input || i.client.is_some() || i.clock_tick || cursor_moved || settle;
        let frame_start = crate::io::rdtsc();
        let cpu_start = trace::cpu_now();

        // CURSOR INVARIANT (see `kitsune_core::cursor`): the sprite lives only in the
        // framebuffer, never in `back`. Every frame that renders anything first erases it,
        // does its own uploads, toasts and HUD, and paints it again as the very last step.
        let mut erased_hud = false;
        if work && let Some(r) = self.passes.erase_cursor(scr) {
            erased_hud = FbPasses::hud_rect(scr.info.width as i32)
                .intersection(&r)
                .is_some();
        }

        let mut path = None;
        let mut hud_wiped = erased_hud;
        if reference || due {
            let cause = Cause {
                input: i.input,
                hover,
                tick,
                settle,
                clock: i.clock_tick,
            };
            let t = trace::t();
            let plan = self.compose(desk, scr, i, cause, reference);
            trace::stage(trace::Stage::Compose, t);
            if let Some(plan) = plan {
                path = Some(classify(&plan, &cause, reference, scr));
                scr.upload_region(&plan.damage);
                hud_wiped |= plan
                    .damage
                    .intersects(&FbPasses::hud_rect(scr.info.width as i32));
            }
        }
        self.was_anim = any_anim;

        if work && path.is_none() && !due && !reference {
            path = Some(trace::Path::Cursor);
        }
        if work {
            perf.record(crate::io::rdtsc().wrapping_sub(frame_start));
            if let Some(p) = path {
                trace::frame(p, frame_start, cpu_start);
            }
        }

        // Toasts, then the HUD, then the cursor: all framebuffer-only, restored from `back`.
        let toast_changed = desk.poll_toasts(crate::klog::ticks_to_ms_now());
        let toast_drawn = self.passes.toasts(desk, scr, toast_changed, work);
        let hud_drawn = self.passes.hud(desk, scr, perf, i.tick, hud_wiped);
        if work || toast_drawn || hud_drawn {
            self.passes.paint_cursor(desk, scr);
        }
        FrameOut { worked: work }
    }

    /// Bring `back` up to date for `desk` and return what changed (`None`: nothing to do).
    fn compose(
        &mut self,
        desk: &mut Desktop,
        scr: &mut Screen,
        i: &FrameIn,
        cause: Cause,
        reference: bool,
    ) -> Option<Plan> {
        if i.force_full {
            self.engine.invalidate_all();
        }
        if let Some(r) = i.extra_dirty {
            self.engine.invalidate(r);
        }
        self.invalidate_for(desk, i, cause);
        // The chrome (panel, app bar, shell layers) is repainted when it may have changed; a
        // frame that only moves windows leaves it alone, which keeps drags cheap.
        let overlay_anim = desk.overlay_open() && desk.shell_animating();
        if cause.input || cause.settle || overlay_anim {
            self.epochs.chrome += 1;
            self.epochs.overlay += 1;
        } else if cause.hover {
            self.epochs.overlay += 1;
        }
        let built = desk.build_scene(self.epochs);
        // A glide inside live windows is smooth at 50 fps: skip the timer ticks in between.
        if built.live_only
            && cause.tick
            && !cause.input
            && !cause.hover
            && !cause.clock
            && !cause.settle
            && i.tick.wrapping_sub(self.live_tick) < 5
        {
            return None;
        }
        if cause.tick && built.live_only {
            self.live_tick = i.tick;
        }
        self.dynamic_prev = built.dynamic;
        self.prev_focused = desk.focused();
        let plan = if reference {
            self.engine.plan_reference(&built.scene)
        } else {
            self.engine.plan(&built.scene)
        };
        let mut painter = paint::DeskPainter {
            desk,
            back: scr.back,
            bg: scr.bg,
            info: scr.info,
            time: i.time,
        };
        plan.paint(&mut painter);
        desk.overlay_painted();
        self.verify
            .check(desk, scr, i.time, &built.scene, &self.engine);
        Some(plan)
    }

    /// Damage that the scene description cannot express: windows that were changed by an
    /// input handler without saying how, the second tick of the clock, windows that just stopped
    /// animating.
    fn invalidate_for(&mut self, desk: &Desktop, i: &FrameIn, cause: Cause) {
        let eng = &mut self.engine;
        if let Some(w) = i
            .client
            .and_then(|id| desk.wm.get(id))
            .filter(|w| w.shown())
        {
            eng.invalidate(Desktop::client_rect(desk.window_box(w)));
        }
        let mut window = |id: Option<WindowId>| {
            if let Some(w) = id.and_then(|id| desk.wm.get(id)).filter(|w| w.shown()) {
                eng.invalidate(desk.window_box(w));
            }
        };
        if cause.input {
            window(desk.focused());
            window(self.prev_focused);
            window(desk.hover);
        }
        if i.external {
            window(desk.browser_id());
        }
        if cause.settle {
            for id in &self.dynamic_prev {
                window(Some(*id));
            }
        }
        if cause.clock {
            eng.invalidate(desk.clock_rect());
            for w in desk.wm.windows() {
                if w.app.kind().is_live() && w.shown() {
                    eng.invalidate(desk.window_box(w));
                }
            }
        }
    }
}

/// Which `trace` bucket a frame goes in (the names the perf tools know).
fn classify(plan: &Plan, c: &Cause, reference: bool, scr: &Screen) -> trace::Path {
    let screen = (scr.info.width * scr.info.height) as u64;
    let full = plan.damage.area() * 10 >= screen * 9;
    if reference {
        trace::Path::Settle
    } else if c.tick {
        if full {
            trace::Path::AnimRebuild
        } else {
            trace::Path::AnimDamage
        }
    } else if c.clock && !c.input {
        trace::Path::ClockLocal
    } else if c.hover && !c.input {
        trace::Path::OverlayHover
    } else if full {
        trace::Path::Settle
    } else {
        trace::Path::Steady
    }
}
