//! Windows of ring 3 processes (`SYS_WIN_*`, see `kitsune_userabi`).
//!
//! The desktop only knows apps by their handle in this table, so a process that wants a window
//! gets a slot **without a wasm runtime**: its frames arrive through [`native_present`] (the
//! `procfsd` thread copies them out of the process), its input leaves through `userproc::win_push`
//! (see `push_ev`), and `appd` never runs it. The compositor, the title bar, minimising, resizing,
//! Tarefas and closing all work as for a WebAssembly app.

use super::*;
use crate::userproc::{self, End};
use kitsune_core::appmanifest::Manifest;
use kitsune_userabi as abi;

/// Windows the desktop has not opened yet (handles).
static PENDING: crate::allocator::SpinLock<Vec<AppId>> =
    crate::allocator::SpinLock::new(Vec::new());

/// A slot for process `pid`: name `name` (the program), initial title and content size.
pub(crate) fn launch_native(
    pid: u32,
    name: &str,
    title: &str,
    w: i32,
    h: i32,
) -> Result<AppId, LaunchError> {
    let mut manifest = Manifest::legacy(&alloc::format!("proc.{name}"), title);
    let (cw, ch) = (w.clamp(1, 1280), h.clamp(1, 800));
    manifest.win_w = cw as u32;
    manifest.win_h = ch as u32;
    manifest.win_min_w = 160;
    manifest.win_min_h = 120;
    manifest.resizable = true;
    let mut slot = make_slot(manifest, false, Arc::from(Vec::<u8>::new()), cw, ch);
    slot.state = State::Running;
    slot.rt_dropped = true;
    slot.want_redraw = false;
    slot.native = Some(pid);
    slot.native_size = (cw, ch);
    slot.title = String::from(title);
    let id = slot.id;
    let ok = with(|slots| match slots.iter_mut().find(|s| s.is_none()) {
        Some(free) => {
            *free = Some(slot);
            true
        }
        None => false,
    });
    if !ok {
        return Err(LaunchError::Full);
    }
    PENDING.lock().push(id);
    Ok(id)
}

/// The windows asked for since the last call, for the desktop to open (compositor thread).
pub(crate) fn take_native_windows() -> Vec<AppId> {
    core::mem::take(&mut *PENDING.lock())
}

/// What the desktop needs to open the window of slot `id`: the program, the title and the content size.
pub(crate) fn native_info(id: AppId) -> Option<(String, String, i32, i32)> {
    with(|slots| {
        let s = slot_of(slots, id)?;
        s.native?;
        Some((s.manifest.id.clone(), s.title.clone(), s.req_w, s.req_h))
    })
}

/// Change the title of the window of slot `id`.
pub(crate) fn native_title(id: AppId, title: &str) {
    with(|slots| {
        if let Some(s) = slot_of(slots, id) {
            s.title = String::from(title);
        }
    });
}

/// The next frame of slot `id`: `w` x `h` pixels, RGBA, tightly packed. Runs on the `procfsd`
/// thread. `false` if the slot is gone or the size is not acceptable.
pub(crate) fn native_present(id: AppId, w: i32, h: i32, rgba: &[u8]) -> bool {
    if w < 1 || h < 1 || w > abi::WIN_MAX_W as i32 || h > abi::WIN_MAX_H as i32 {
        return false;
    }
    let (wu, hu) = (w as usize, h as usize);
    if rgba.len() < wu * hu * 4 {
        return false;
    }
    let Some(info) = surface_info(w, h) else {
        return false;
    };
    let bpp = info.bytes_per_pixel;
    // Convert outside the lock, into the screen's own pixel layout.
    let mut native: Vec<u8> = Vec::new();
    if native.try_reserve_exact(wu * hu * bpp).is_err() {
        return false;
    }
    native.resize(wu * hu * bpp, 0);
    let bgr = matches!(info.pixel_format, bootloader_api::info::PixelFormat::Bgr);
    for (src, dst) in rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(native.chunks_exact_mut(bpp))
    {
        let (r, g, b) = (src[0], src[1], src[2]);
        if bgr {
            dst[0] = b;
            dst[1] = g;
            dst[2] = r;
        } else {
            dst[0] = r;
            dst[1] = g;
            dst[2] = b;
        }
    }
    let Some(i) = with(|slots| {
        slots
            .iter()
            .position(|s| s.as_ref().is_some_and(|s| s.id == id && s.native.is_some()))
    }) else {
        return false;
    };
    // A new size: both surfaces are made again (waits for the compositor to finish reading one).
    let same = with(|slots| {
        slots[i]
            .as_ref()
            .is_some_and(|s| (s.cur_w, s.cur_h) == (w, h) && !s.surf[0].is_empty())
    });
    if !same && !runtime::resize_surfaces(i, w, h) {
        return false;
    }
    // Into the back surface a band of rows at a time, so interrupts are never off for long.
    let back = with(|slots| {
        let s = slots[i].as_ref()?;
        (s.id == id).then_some(1 - s.front)
    });
    let Some(back) = back else { return false };
    const BAND: usize = 32 * 1024;
    let mut at = 0;
    while at < native.len() {
        let end = (at + BAND).min(native.len());
        let ok = with(|slots| match slots[i].as_mut() {
            Some(s)
                if s.id == id && s.surf[back].len() == native.len() && s.reading != Some(back) =>
            {
                s.surf[back][at..end].copy_from_slice(&native[at..end]);
                true
            }
            _ => false,
        });
        if !ok {
            return false;
        }
        at = end;
    }
    let now = crate::interrupts::ticks();
    with(|slots| {
        if let Some(s) = slots[i].as_mut()
            && s.id == id
            && s.surf[back].len() == native.len()
        {
            s.front = back;
            s.ready = true;
            s.frames += 1;
            s.last_frame = now;
        }
    });
    true
}

/// Has a process behind a window ended since the last look? (`appd` blocks on this too)
pub(super) fn any_ended() -> bool {
    ended()
        .iter()
        .any(|(_, _, end)| !matches!(end, End::Running))
}

fn ended() -> Vec<(usize, u32, End)> {
    let pids: Vec<(usize, u32)> = with(|slots| {
        slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let s = s.as_ref()?;
                (s.state == State::Running || s.state == State::Suspended).then_some((i, s.native?))
            })
            .collect()
    });
    pids.into_iter()
        .map(|(i, pid)| (i, pid, userproc::end(pid)))
        .collect()
}

/// A window whose process ended shows why, as a WebAssembly app does.
pub(super) fn reap_ended() {
    for (i, pid, end) in ended() {
        let (state, why) = match end {
            End::Running => continue,
            End::Exited(code) => (
                State::Exited,
                Why::with(tk!("apps.why.exit_code"), alloc::format!("{code}")),
            ),
            End::Killed => (State::Crashed, Why::new(tk!("apps.why.proc_died"))),
        };
        let _ = pid;
        with(|slots| {
            if let Some(s) = slots[i].as_mut() {
                serial_println!(
                    "apps: process window `{}` ended: {}",
                    s.manifest.id,
                    why.log()
                );
                s.state = state;
                s.reason = why;
                s.events.clear();
            }
        });
    }
}
