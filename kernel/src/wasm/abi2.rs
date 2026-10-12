//! ABI v2: the `osj.*` host functions (`osj` is a historical abbreviation, kept as the
//! import module name so already compiled apps keep linking).
//!
//! Every function validates what the guest hands it before touching anything
//! (`kitsune_core::appabi::check_range`): a pointer/length outside the guest's
//! linear memory raises an [`AppFault`] trap and **only that app dies** ("ponteiro
//! invalido"); a length above the function's cap returns `ERR_INVAL`; a denied
//! permission returns `ERR_PERM`. Host work the fuel meter cannot see (pixel
//! fills, image decoding, file I/O) is charged to the guest's fuel.
//!
//! The decisions (paths, quotas, descriptors, URL filter) live in
//! `kitsune_core::{appfs, appnet}`; this file only moves bytes between the guest's
//! memory and those pure pieces.

use super::{AppFault, HostState, appfs_backend, guest_mem, host_fill, host_text, manager};
use crate::interrupts;
use alloc::string::String;
use alloc::vec::Vec;
use core::ops::Range;
use kitsune_core::appabi::remote::{HOST_FNS, HOST_MODULE, Ty};
use kitsune_core::appabi::*;
use kitsune_core::appfs::Sandbox;
use kitsune_core::appmanifest::{ClipPerm, NetPerm};
use kitsune_core::appnet;
use wasmi::{Caller, FuncType, Linker, Val, ValType};

mod guest;
mod hostio;
mod pixels;
pub(crate) use guest::*;
use hostio::*;
use pixels::*;

type R<T> = Result<T, wasmi::Error>;

/// Per-app v2 state, in the `Store`'s `HostState`.
pub(crate) struct V2 {
    pub id: String,
    pub sandbox: Sandbox,
    pub net: NetPerm,
    /// The manifest's `net_hosts` allow-list (empty: any public host).
    pub net_hosts: Vec<String>,
    pub clip: ClipPerm,
    pub title: String,
    pub title_dirty: bool,
    /// The guest asked for a redraw (`request_redraw`).
    pub redraw: bool,
    pub log_left: usize,
    pub limiter: appnet::Limiter,
    /// Monotonic ms at start, and the wall clock (ms since local midnight) then.
    pub mono0_ms: u64,
    pub wall0_ms: u64,
    /// Path-escape attempts already reported to the serial log.
    pub escapes_logged: u32,
}
impl V2 {
    pub(crate) fn new(
        id: &str,
        sandbox: Sandbox,
        net: NetPerm,
        net_hosts: Vec<String>,
        clip: ClipPerm,
        mono_ms: u64,
        wall_ms: u64,
    ) -> V2 {
        V2 {
            id: String::from(id),
            sandbox,
            net,
            net_hosts,
            clip,
            title: String::new(),
            title_dirty: false,
            redraw: false,
            log_left: LOG_BUDGET,
            limiter: appnet::Limiter::new(),
            mono0_ms: mono_ms,
            wall0_ms: wall_ms,
            escapes_logged: 0,
        }
    }
}
fn val_ty(t: Ty) -> ValType {
    match t {
        Ty::I32 => ValType::I32,
        Ty::I64 => ValType::I64,
    }
}
/// Register every `osj.*` function: each one forwards to [`call`] with its wire id.
pub(crate) fn install(l: &mut Linker<HostState>) -> Result<(), &'static str> {
    for (id, f) in HOST_FNS.iter().enumerate() {
        let ty = FuncType::new(
            f.params.iter().map(|t| val_ty(*t)),
            f.results.iter().map(|t| val_ty(*t)),
        );
        l.func_new(
            HOST_MODULE,
            f.name,
            ty,
            move |c: Caller<'_, HostState>, params: &[Val], results: &mut [Val]| {
                let mut a = [0i64; 6];
                for (slot, v) in a.iter_mut().zip(params) {
                    *slot = match v {
                        Val::I32(x) => i64::from(*x),
                        Val::I64(x) => *x,
                        _ => 0,
                    };
                }
                let mut g = Local(c, core::marker::PhantomData);
                let r = call(&mut g, id, &a)?;
                if let (Some(slot), Some(t)) = (results.first_mut(), HOST_FNS[id].results.first()) {
                    *slot = match t {
                        Ty::I32 => Val::I32(r as i32),
                        Ty::I64 => Val::I64(r),
                    };
                }
                Ok(())
            },
        )
        .map_err(|_| "link osj")?;
    }
    Ok(())
}
/// Run host function `id` of `HOST_FNS` with `a` (each `i32` sign-extended). The value is the
/// function's result (`0` for none).
pub(crate) fn call(g: &mut dyn Guest, id: usize, a: &[i64; 6]) -> R<i64> {
    let i = |n: usize| a[n] as i32;
    let name = HOST_FNS.get(id).map_or("", |f| f.name);
    let v: i32 = match name {
        // ------------------------------------------------------------ window
        "set_title" => {
            let bytes = match read_capped(g, i(0), i(1), MAX_TITLE)? {
                Ok(b) => b,
                Err(e) => return Ok(i64::from(e)),
            };
            if !bytes.iter().all(|&b| (0x20..=0x7E).contains(&b)) {
                return Ok(i64::from(ERR_INVAL));
            }
            let st = v2(g)?;
            st.title = alloc::string::String::from_utf8_lossy(&bytes).into_owned();
            st.title_dirty = true;
            0
        }
        "get_size" => {
            let st = g.host();
            return Ok(((st.cw as i64) << 32) | (st.ch as u32 as i64));
        }
        "request_redraw" => {
            v2(g)?.redraw = true;
            0
        }
        // ------------------------------------------------------------ drawing
        "fill_rect" => {
            let (x, y, w, h, color) = (i(0), i(1), i(2), i(3), i(4));
            if w > 0 && h > 0 {
                g.charge((w as u64 * h as u64 / 256 + 1).min(1 << 20))?;
            }
            host_fill(g.host(), x, y, w, h, color);
            0
        }
        "draw_text" => {
            let bytes = match read_capped(g, i(2), i(3), MAX_TEXT)? {
                Ok(b) => b,
                Err(e) => return Ok(i64::from(e)),
            };
            g.charge(bytes.len() as u64 * 8)?;
            // UTF-8 text (accents included); bytes that are not UTF-8 are read as Latin-1, as old
            // apps wrote them. Never an error.
            let s: String = match core::str::from_utf8(&bytes) {
                Ok(s) => String::from(s),
                Err(_) => bytes.iter().map(|&b| b as char).collect(),
            };
            host_text(g.host(), &s, i(0), i(1), i(4), i(5));
            0
        }
        // The language of the interface: 0 Portuguese (Brazil), 1 English. An app asks again each
        // time it draws, because the language can change while it runs.
        "lang" => i32::from(kitsune_core::i18n::lang().index()),
        "blit_rgba" => {
            let (ptr, w, h, dx, dy) = (i(0), i(1), i(2), i(3), i(4));
            if w <= 0 || h <= 0 || (w as u64) * (h as u64) > MAX_BLIT_PIXELS {
                return Ok(i64::from(ERR_INVAL));
            }
            let r = range(g, ptr, w * h * 4)?;
            g.charge((w as u64) * (h as u64) / 8)?;
            let px = g.read_mem(r);
            blit_1to1(g.host_mut(), &px, w, h, dx, dy);
            0
        }
        "draw_image_png" => {
            let bytes = match read_capped(g, i(0), i(1), MAX_PNG_BYTES)? {
                Ok(b) => b,
                Err(e) => return Ok(i64::from(e)),
            };
            let Ok(h) = kitsune_core::png::read_header(&bytes) else {
                return Ok(i64::from(ERR_INVAL));
            };
            if h.width > MAX_PNG_DIM || h.height > MAX_PNG_DIM {
                return Ok(i64::from(ERR_INVAL));
            }
            g.charge((h.width as u64) * (h.height as u64) / 2 + 1000)?;
            let Ok(img) = kitsune_core::png::decode(&bytes) else {
                return Ok(i64::from(ERR_INVAL));
            };
            let Ok(rgba) = img.to_rgba() else {
                return Ok(i64::from(ERR_INVAL));
            };
            blit_alpha(
                g.host(),
                &rgba,
                img.width() as i32,
                img.height() as i32,
                i(2),
                i(3),
            );
            ((img.width() as i32) << 16) | img.height() as i32
        }
        // ------------------------------------------------------------ time, random, log, exit
        "now_ms" => {
            let st = v2(g)?;
            return Ok((st.wall0_ms + mono_ms().saturating_sub(st.mono0_ms)) as i64);
        }
        "monotonic_ms" => return Ok(mono_ms() as i64),
        // Random bits come from the kernel generator (`crate::rng`), not a per-app xorshift.
        "random" => crate::rng::u32() as i32,
        "log" => {
            let bytes = match read_capped(g, i(0), i(1), MAX_LOG)? {
                Ok(b) => b,
                Err(e) => return Ok(i64::from(e)),
            };
            let st = v2(g)?;
            if bytes.len() > st.log_left {
                st.log_left = 0;
                return Ok(i64::from(ERR_NOSPC));
            }
            st.log_left -= bytes.len();
            // Printable ASCII only: a guest cannot inject escape sequences into the log.
            let s: String = bytes
                .iter()
                .map(|&b| {
                    if (0x20..=0x7E).contains(&b) {
                        b as char
                    } else {
                        '.'
                    }
                })
                .collect();
            crate::serial_println!("[app {}] {}", st.id, s);
            0
        }
        "exit" => return Err(wasmi::Error::i32_exit(i(0))),
        // ------------------------------------------------------------ clipboard
        "clip_get" => {
            if v2(g)?.clip != ClipPerm::Rw {
                return Ok(i64::from(ERR_PERM));
            }
            let cap = i(1).clamp(0, 256);
            let r = range(g, i(0), cap)?;
            let mut tmp = [0u8; 256];
            let n = manager::clip_get(&mut tmp[..r.len()]);
            write_guest(g, i(0), &tmp[..n])?;
            n as i32
        }
        "clip_set" => {
            if v2(g)?.clip != ClipPerm::Rw {
                return Ok(i64::from(ERR_PERM));
            }
            let bytes = match read_capped(g, i(0), i(1), 256)? {
                Ok(b) => b,
                Err(e) => return Ok(i64::from(e)),
            };
            manager::clip_set(&bytes);
            0
        }
        // ------------------------------------------------------------ files
        "fs_open" => {
            let path = match read_capped(g, i(0), i(1), MAX_PATH_ARG)? {
                Ok(b) => b,
                Err(e) => return Ok(i64::from(e)),
            };
            let st = v2(g)?;
            let r = appfs_backend::try_with(|fs| st.sandbox.open(fs, &path, i(2) as u32));
            let code = res(r);
            report_escapes(st);
            g.charge(200)?;
            code
        }
        "fs_close" => {
            let st = v2(g)?;
            res(st.sandbox.close(i(0)).map(|()| 0))
        }
        "fs_read" => {
            if i(2) < 0 {
                return Err(fault(kitsune_core::tk!("apps.why.bad_pointer")));
            }
            let want = (i(2) as usize).min(MAX_IO);
            let r = range(g, i(1), want as i32)?;
            let mut buf = alloc::vec![0u8; r.len()];
            let fd = i(0);
            let v = v2(g)?;
            let out = appfs_backend::try_with(|fs| v.sandbox.read(fs, fd, &mut buf));
            if let Ok(n) = out {
                g.write_mem(r.start, &buf[..n]);
            }
            let code = res(out.map(|n| n as i32));
            g.charge(100 + want as u64 / 64)?;
            code
        }
        "fs_write" => {
            if i(2) < 0 {
                return Err(fault(kitsune_core::tk!("apps.why.bad_pointer")));
            }
            let want = (i(2) as usize).min(MAX_IO);
            let r = range(g, i(1), want as i32)?;
            let data = g.read_mem(r);
            let fd = i(0);
            let v = v2(g)?;
            let out = appfs_backend::try_with(|fs| v.sandbox.write(fs, fd, &data));
            let code = res(out.map(|n| n as i32));
            g.charge(100 + want as u64 / 64)?;
            code
        }
        "fs_seek" => {
            let st = v2(g)?;
            let (fd, off, whence) = (i(0), a[1], i(2));
            let r = appfs_backend::try_with(|fs| st.sandbox.seek(fs, fd, off, whence));
            return Ok(match r {
                Ok(p) => p as i64,
                Err(e) => i64::from(e.code()),
            });
        }
        "fs_stat" => {
            let path = match read_capped(g, i(0), i(1), MAX_PATH_ARG)? {
                Ok(b) => b,
                Err(e) => return Ok(i64::from(e)),
            };
            range(g, i(2), 16)?; // validate before doing the work
            let st = v2(g)?;
            let r = appfs_backend::try_with(|fs| st.sandbox.stat(fs, &path));
            report_escapes(st);
            match r {
                Ok(s) => {
                    let mut b = [0u8; 16];
                    b[..4].copy_from_slice(&(s.kind as u32).to_le_bytes());
                    b[4..12].copy_from_slice(&s.size.to_le_bytes());
                    write_guest(g, i(2), &b)?;
                    0
                }
                Err(e) => e.code(),
            }
        }
        "fs_readdir" => {
            let path = match read_capped(g, i(0), i(1), MAX_PATH_ARG)? {
                Ok(b) => b,
                Err(e) => return Ok(i64::from(e)),
            };
            let (index, out, cap) = (i(2), i(3), i(4));
            if !(2..=256).contains(&cap) {
                return Ok(i64::from(ERR_INVAL));
            }
            range(g, out, cap)?;
            if index < 0 {
                return Ok(i64::from(ERR_INVAL));
            }
            let st = v2(g)?;
            let r = appfs_backend::try_with(|fs| st.sandbox.read_dir(fs, &path, index as usize));
            report_escapes(st);
            match r {
                Ok(Some(e)) => {
                    let name = e.name.as_bytes();
                    if name.len() + 1 > cap as usize {
                        return Ok(i64::from(ERR_INVAL));
                    }
                    let mut b = Vec::with_capacity(name.len() + 1);
                    b.push(e.kind as u8);
                    b.extend_from_slice(name);
                    write_guest(g, out, &b)?;
                    name.len() as i32
                }
                Ok(None) => 0,
                Err(e) => e.code(),
            }
        }
        "fs_mkdir" => path_op(g, i(0), i(1), |sb, fs, path| sb.mkdir(fs, path))?,
        "fs_unlink" => path_op(g, i(0), i(1), |sb, fs, path| sb.unlink(fs, path))?,
        "fs_rename" => {
            let from = match read_capped(g, i(0), i(1), MAX_PATH_ARG)? {
                Ok(b) => b,
                Err(e) => return Ok(i64::from(e)),
            };
            let to = match read_capped(g, i(2), i(3), MAX_PATH_ARG)? {
                Ok(b) => b,
                Err(e) => return Ok(i64::from(e)),
            };
            let st = v2(g)?;
            let r = appfs_backend::try_with(|fs| st.sandbox.rename(fs, &from, &to));
            report_escapes(st);
            res(r.map(|()| 0))
        }
        // ------------------------------------------------------------ network
        "net_http_get" => net_http_get(g, i(0), i(1), i(2), i(3))?,
        _ => return Err(fault(kitsune_core::tk!("apps.why.no_abi2"))),
    };
    Ok(i64::from(v))
}
