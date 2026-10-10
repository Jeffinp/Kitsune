//! The sign-in and lock screen.
//!
//! A full-screen layer (see `compositor/layers.rs`): the wallpaper under a dim, a card with the
//! account (or one chip per account when there is a choice), a password field and the button.
//! It takes every key and click while it is up, so nothing behind it can be reached. Signing in
//! asks `services::accounts`; a wrong password clears the field and says so, and too many wrong
//! ones are slowed down by the account layer (the message tells how long to wait).

use super::dialog::R_PANEL;
use crate::desktop::kit::appui::{self, FieldText};
use crate::desktop::services::accounts::{self, LoginFail};
use crate::desktop::shell::*;
use crate::desktop::*;
use crate::text::{self, BODY, FOOTNOTE, TITLE2, Weight};
use kitsune_core::t;

/// Longest password the field takes.
const MAX_PW: usize = 64;

/// Where the card's parts are.
pub(super) struct LoginGeom {
    pub panel: Rect,
    pub avatar: Rect,
    pub chips: Vec<Rect>,
    pub field: Rect,
    pub button: Rect,
}

pub(super) fn login_geom(sw: i32, sh: i32, users: usize) -> LoginGeom {
    let chips_h = if users > 1 { 64 } else { 0 };
    // The hint under the card exists only with several accounts.
    let h = if users > 1 { 356 } else { 330 } + chips_h;
    let panel = Rect::new(sw / 2 - 190, sh / 2 - h / 2, 380, h);
    let mut chips = Vec::new();
    if users > 1 {
        let n = users.min(6) as i32;
        let total = n * 44 + (n - 1) * 12;
        let x0 = panel.x + (panel.w - total) / 2;
        for i in 0..n {
            chips.push(Rect::new(x0 + i * 56, panel.y + 18, 44, 44));
        }
    }
    let avatar = Rect::new(panel.x + (panel.w - 76) / 2, panel.y + 28 + chips_h, 76, 76);
    let field = Rect::new(panel.x + 40, panel.bottom() - 158, panel.w - 80, 38);
    let button = Rect::new(panel.x + (panel.w - 130) / 2, panel.bottom() - 106, 130, 34);
    LoginGeom {
        panel,
        avatar,
        chips,
        field,
        button,
    }
}

fn initial(name: &str) -> String {
    name.chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default()
}

impl Desktop {
    /// Show the screen. `locked`: keep the session and its windows (only its own account can
    /// sign in); otherwise it is a fresh sign-in with every account to choose from.
    pub(crate) fn open_login(&mut self, locked: bool) {
        self.close_transients();
        self.shell.search = None;
        self.shell.apps = None;
        self.shell.dialog = None;
        let mut users = accounts::users();
        let me = accounts::current().map(|s| s.name);
        if locked && let Some(me) = &me {
            users.retain(|u| &u.name == me);
        }
        if users.is_empty() {
            return;
        }
        let sel = me
            .and_then(|m| users.iter().position(|u| u.name == m))
            .unwrap_or(0);
        self.shell.login = Some(LoginView {
            users,
            sel,
            pw: String::new(),
            msg: None,
            locked,
            t: fade_in(OVERLAY_FADE),
            closing: false,
        });
        self.force_full = true;
    }

    /// Is the screen up (and not already leaving)?
    pub(crate) fn login_active(&self) -> bool {
        self.shell.login.as_ref().is_some_and(|l| !l.closing)
    }

    pub(super) fn draw_login(&self, c: &mut Canvas, l: &LoginView) {
        let p = theme::pal();
        let fade = level(&l.t);
        let full = Rect::new(0, 0, self.sw, self.sh);
        c.blend_rect(full, Color::rgb(0, 0, 0), (150 * fade / 256) as u16);
        let g = login_geom(self.sw, self.sh, l.users.len());
        let hole = Rect::new(g.panel.x, g.panel.y + 14, g.panel.w, g.panel.h - 28);
        c.draw_shadow(
            g.panel,
            Shadow {
                blur: 32,
                dy: 18,
                alpha: (120 * fade / 256).min(255),
            },
            hole,
        );
        c.fill_rrect(
            g.panel,
            R_PANEL,
            Corner::Circle,
            theme::solid(p.window_bg),
            fade as u16,
        );
        ui::stroke_token(c, g.panel, R_PANEL, p.separator);
        if fade < 150 {
            return;
        }
        // One chip per account when there is a choice.
        for (i, r) in g.chips.iter().enumerate() {
            let Some(u) = l.users.get(i) else { break };
            let on = i == l.sel;
            if on {
                c.fill_rrect(r.inflated(4), 26, Corner::Circle, theme::accent(), 90);
            }
            c.fill_rrect(
                *r,
                22,
                Corner::Circle,
                if on {
                    theme::accent()
                } else {
                    theme::solid(p.text_tertiary)
                },
                256,
            );
            text::draw_centered(
                c,
                *r,
                &initial(accounts::display_name(u)),
                BODY,
                Weight::Semibold,
                Color::rgb(255, 255, 255),
            );
        }
        let Some(u) = l.users.get(l.sel) else {
            return;
        };
        // The avatar and who it is.
        c.fill_rrect(g.avatar, 38, Corner::Circle, theme::accent(), 256);
        text::draw_centered(
            c,
            g.avatar,
            &initial(accounts::display_name(u)),
            TITLE2,
            Weight::Semibold,
            Color::rgb(255, 255, 255),
        );
        let name_r = Rect::new(g.panel.x + 20, g.avatar.bottom() + 10, g.panel.w - 40, 28);
        text::draw_centered(
            c,
            name_r,
            accounts::display_name(u),
            TITLE2,
            Weight::Semibold,
            theme::solid(p.text),
        );
        let role = if l.locked {
            t!("login.locked")
        } else if u.admin {
            t!("login.admin")
        } else {
            t!("login.user")
        };
        text::draw_centered(
            c,
            Rect::new(name_r.x, name_r.bottom(), name_r.w, 20),
            role,
            FOOTNOTE,
            Weight::Regular,
            theme::solid(p.text_secondary),
        );
        // The password field: bullets only.
        let bullets: String = "•".repeat(l.pw.len());
        appui::field(
            c,
            g.field,
            &FieldText {
                text: &bullets,
                caret: bullets.len(),
                selection: None,
            },
            if u.has_password {
                t!("login.password")
            } else {
                t!("login.no_password")
            },
            true,
            256,
            None,
            false,
        );
        ui::push_button(
            c,
            g.button,
            t!("login.enter"),
            ui::ButtonKind::Primary,
            ui::Control::Normal,
        );
        if let Some((m, err)) = &l.msg {
            text::draw_centered(
                c,
                Rect::new(g.panel.x + 20, g.button.bottom() + 8, g.panel.w - 40, 20),
                m,
                FOOTNOTE,
                Weight::Regular,
                if *err {
                    theme::solid(0xFF_E5_48_4D)
                } else {
                    theme::solid(p.text_secondary)
                },
            );
        }
        if l.users.len() > 1 {
            text::draw_centered(
                c,
                Rect::new(g.panel.x, g.panel.bottom() - 28, g.panel.w, 18),
                t!("login.hint"),
                FOOTNOTE,
                Weight::Regular,
                theme::solid(p.text_tertiary),
            );
        }
    }

    /// A key while the screen is up (it takes all of them).
    pub(crate) fn login_key(&mut self, key: Key) {
        let Some(l) = self.shell.login.as_mut() else {
            return;
        };
        if l.closing {
            return;
        }
        match key {
            Key::Char(ch) if (0x20..0x7F).contains(&ch) => {
                if l.pw.len() < MAX_PW {
                    l.pw.push(ch as char);
                }
                l.msg = None;
            }
            Key::Backspace => {
                l.pw.pop();
                l.msg = None;
            }
            Key::Tab | Key::Right | Key::Down => {
                if !l.users.is_empty() {
                    l.sel = (l.sel + 1) % l.users.len();
                    l.pw.clear();
                    l.msg = None;
                }
            }
            Key::Left | Key::Up => {
                if !l.users.is_empty() {
                    l.sel = (l.sel + l.users.len() - 1) % l.users.len();
                    l.pw.clear();
                    l.msg = None;
                }
            }
            Key::Enter => {
                self.login_submit();
            }
            _ => {}
        }
        self.force_full = true;
    }

    /// A click while the screen is up.
    pub(crate) fn login_click(&mut self, x: i32, y: i32) {
        let Some(l) = self.shell.login.as_ref().filter(|l| !l.closing) else {
            return;
        };
        let g = login_geom(self.sw, self.sh, l.users.len());
        if g.button.contains(x, y) {
            self.login_submit();
        } else if let Some(i) = g.chips.iter().position(|r| r.contains(x, y))
            && let Some(l) = self.shell.login.as_mut()
            && i < l.users.len()
            && i != l.sel
        {
            l.sel = i;
            l.pw.clear();
            l.msg = None;
        }
        self.force_full = true;
    }

    /// Check the password of the chosen account.
    fn login_submit(&mut self) {
        let Some(l) = self.shell.login.as_ref().filter(|l| !l.closing) else {
            return;
        };
        let Some(user) = l.users.get(l.sel) else {
            return;
        };
        let (name, pw, locked) = (user.name.clone(), l.pw.clone(), l.locked);
        let r = accounts::login(&name, &pw);
        match r {
            Ok(()) => {
                if let Some(l) = self.shell.login.as_mut() {
                    l.closing = true;
                    l.pw.clear();
                    fade_out(&mut l.t, OVERLAY_FADE);
                }
                if !locked {
                    self.after_sign_in();
                }
            }
            Err(e) => {
                let msg = match e {
                    LoginFail::Unknown | LoginFail::BadPassword => String::from(t!("login.bad")),
                    LoginFail::Locked => String::from(t!("login.locked_account")),
                    LoginFail::Throttled(ms) => {
                        t!("login.throttled", s = (ms as i64 + 999) / 1000)
                    }
                };
                if let Some(l) = self.shell.login.as_mut() {
                    l.pw.clear();
                    l.msg = Some((msg, true));
                }
            }
        }
        self.force_full = true;
    }

    /// A fresh session starts: the terminal is open and focused, as at the first boot.
    fn after_sign_in(&mut self) {
        if self.wm.windows().iter().all(|w| w.is_closing()) {
            self.open_new(Kind::Terminal);
        }
    }

    /// `Cmd::Lock`.
    pub(crate) fn lock_screen(&mut self) {
        if accounts::signed_in() {
            self.open_login(true);
        }
    }

    /// Lock the screen after `Settings::auto_lock_min` minutes without input, for an account that
    /// has a password (for one without, the lock screen would only be a click away).
    pub(crate) fn auto_lock_tick(&mut self) {
        let min = crate::settings::get().auto_lock_min;
        if min == 0 || self.login_active() || !accounts::current_has_password() {
            return;
        }
        let idle = crate::netd::now_ms().saturating_sub(self.last_input_ms);
        if idle >= u64::from(min) * 60_000 {
            self.last_input_ms = crate::netd::now_ms();
            crate::klog!(Info, "auth: screen locked after {min} min without input");
            self.lock_screen();
        }
    }

    /// `Cmd::SignOut`: close the windows (an editor with unsaved changes asks first) and show
    /// the sign-in screen.
    pub(crate) fn sign_out(&mut self) {
        if self.guard_unsaved() {
            return;
        }
        let ids: Vec<WindowId> = self
            .wm
            .windows()
            .iter()
            .filter(|w| !w.is_closing())
            .map(|w| w.id)
            .collect();
        for id in ids {
            self.request_close(id);
        }
        accounts::logout();
        self.open_login(false);
    }
}
