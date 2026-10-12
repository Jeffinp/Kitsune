//! The Terminal as a ring 3 program. The shell engine and the line editor are `kitsune_core`'s,
//! the same the desktop's terminal uses; here they run in a process of their own over the file and
//! process calls of the kernel, so everything a command does is checked against the rights of the
//! user who opened the terminal. Run it with `run kterm`.
#![no_std]
#![no_main]

extern crate alloc;

mod child;
mod draw;
mod fsys;
mod sysx;

use alloc::string::String;
use alloc::vec::Vec;
use fsys::UserFs;
use kitsune_core::i18n::{self, Lang};
use kitsune_core::shell::{Host, Shell, ShellFs, Term, TermAction};
use kitsune_core::system::input::{KeyCode, KeyEvent, Mods};
use kitsune_user::abi::{EV_CLOSE, EV_KEY, EV_RESIZE, Event, WIN_MAX_H, WIN_MAX_W};
use kitsune_user::fs::Env;
use kitsune_user::win::{self, Window};
use kitsune_user::*;
use sysx::{Pump, UserSys};

entry!(main);

/// A window of 80 x 26 characters to begin with.
const START_W: u32 = 80 * draw::CELL_W;
const START_H: u32 = 26 * draw::CELL_H;

/// The key a window event stands for (`None` for the ones the line editor has no use for).
fn key_of(ev: &Event) -> Option<KeyEvent> {
    let code = match ev.a {
        8 => KeyCode::Backspace,
        9 => KeyCode::Tab,
        10 | 13 => KeyCode::Enter,
        27 => KeyCode::Esc,
        127 => KeyCode::Delete,
        0x100 => KeyCode::Left,
        0x101 => KeyCode::Right,
        0x102 => KeyCode::Up,
        0x103 => KeyCode::Down,
        0x104 => KeyCode::Home,
        0x105 => KeyCode::End,
        0x106 => KeyCode::PageUp,
        0x107 => KeyCode::PageDown,
        c @ 0x20..0x7F => KeyCode::Char(c as u8 as char),
        _ => return None,
    };
    Some(KeyEvent::new(
        code,
        Mods {
            shift: ev.b & 1 != 0,
            ctrl: ev.b & 2 != 0,
            alt: ev.b & 4 != 0,
        },
    ))
}

fn new_shell() -> Shell {
    let mut sh = Shell::new();
    // The process stack holds 512 KiB; keep the recursion well within what the host tests cover.
    sh.limits.max_call_depth = 16;
    sh.limits.max_sub_depth = 4;
    sh.env.set("PS1", "\\w\\$ ");
    sh.env.set("HOME", "/");
    sh
}

fn main() -> i32 {
    i18n::set_lang(Lang::from_index(language()));
    let env = Env::load();
    let (mut w, mut h) = (START_W, START_H);
    let Ok(window) = Window::open(w, h, "Terminal (ring 3)") else {
        println!("kterm: could not open the window");
        return 1;
    };
    let pump = Pump::new();
    let mut fs = UserFs::new(env.cwd());
    let mut shell = new_shell();
    let mut sys = UserSys { pump: &pump };
    let mut term = Term::new(&shell.prompt(&fs, &sys));
    let (mut cols, mut rows) = draw::grid(w, h);
    term.resize(cols, rows);
    term.print(match i18n::lang() {
        Lang::Pt => "Terminal em anel 3: o shell roda num processo proprio, com os direitos do seu usuario.\n",
        Lang::En => "Ring 3 terminal: the shell runs in a process of its own, with your user's rights.\n",
    });
    let mut frame: Vec<u8> = Vec::new();
    let mut dirty = true;
    loop {
        if dirty {
            let view = term.view_in(cols, rows);
            draw::paint(&mut frame, w, h, &view, cols, term.prompt_chars());
            let _ = window.present(&frame, w, h);
            let _ = window.set_title(&title(&fs));
            dirty = false;
        }
        let ev = match pump.next_queued() {
            Some(e) => e,
            None => match win::next_event(u64::MAX) {
                Ok(Some(e)) => e,
                _ => continue,
            },
        };
        match ev.kind {
            EV_CLOSE => return 0,
            EV_RESIZE => {
                w = (ev.a as u32).clamp(draw::CELL_W * 8, WIN_MAX_W);
                h = (ev.b as u32).clamp(draw::CELL_H * 4, WIN_MAX_H);
                (cols, rows) = draw::grid(w, h);
                term.resize(cols, rows);
                dirty = true;
            }
            EV_KEY => {
                let Some(key) = key_of(&ev) else { continue };
                let action = term.key(key, Some((&shell, &fs)));
                match action {
                    TermAction::None | TermAction::Cancel => {}
                    TermAction::Redraw => dirty = true,
                    TermAction::Exit => return 0,
                    TermAction::Run(line) => {
                        // Show the echoed line before the command (it may take a while).
                        let view = term.view_in(cols, rows);
                        draw::paint(&mut frame, w, h, &view, cols, term.prompt_chars());
                        let _ = window.present(&frame, w, h);
                        pump.arm();
                        let result = {
                            let mut host = Host {
                                fs: &mut fs,
                                sys: &mut sys,
                            };
                            shell.run_line(&line, &mut host)
                        };
                        if pump.closed() {
                            return 0;
                        }
                        let prompt = shell.prompt(&fs, &sys);
                        if term.finish(&result, &prompt) == TermAction::Exit {
                            return result.exit.unwrap_or(0);
                        }
                        dirty = true;
                    }
                }
            }
            _ => {}
        }
    }
}

/// The window title: the working directory.
fn title(fs: &UserFs) -> String {
    let mut t = String::from("Terminal - ");
    t.push_str(&fs.cwd());
    t.truncate(kitsune_user::abi::WIN_MAX_TITLE);
    t
}
