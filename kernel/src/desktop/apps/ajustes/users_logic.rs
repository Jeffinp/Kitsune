//! What the Users page does: forms, typing and the calls into `services::accounts`.

use super::state::*;
use crate::desktop::Key;
use crate::desktop::services::accounts::{self, ChangeFail};
use kitsune_core::account::AccountError;
use kitsune_core::password::PasswordError;
use kitsune_core::tk;

/// Longest name / password typed here (the core validates the rest).
const NAME_MAX: usize = 32;
const FULL_MAX: usize = 40;
const PW_MAX: usize = 64;

fn why(e: &ChangeFail) -> &'static str {
    match e {
        ChangeFail::Denied => tk!("settings.users.err.denied"),
        ChangeFail::BadPassword => tk!("settings.users.err.wrong_pw"),
        ChangeFail::Password(PasswordError::TooShort) => tk!("settings.users.err.short"),
        ChangeFail::Password(_) => tk!("settings.users.err.pw_chars"),
        ChangeFail::Account(AccountError::BadName) => tk!("settings.users.err.name"),
        ChangeFail::Account(AccountError::BadFullName) => tk!("settings.users.err.full"),
        ChangeFail::Account(AccountError::DuplicateName) => tk!("settings.users.err.exists"),
        ChangeFail::Account(AccountError::TooManyUsers) => tk!("settings.users.err.many"),
        ChangeFail::Account(AccountError::NoSuchUser) => tk!("settings.users.err.nobody"),
        ChangeFail::Account(AccountError::Protected) => tk!("settings.users.err.protected"),
        ChangeFail::Account(_) | ChangeFail::Storage => tk!("settings.users.err.storage"),
    }
}

impl SettingsState {
    fn users_result(&mut self, r: Result<(), ChangeFail>, ok: &'static str) -> bool {
        match r {
            Ok(()) => {
                self.say(ok, false);
                true
            }
            Err(e) => {
                self.say(why(&e), true);
                false
            }
        }
    }

    /// The text field behind `Focus::User(k)` and its limit.
    fn user_field(&mut self, k: u8) -> (&mut alloc::string::String, usize) {
        let f = &mut self.users;
        match k {
            0 => (&mut f.name, NAME_MAX),
            1 => (&mut f.full, FULL_MAX),
            2 => (&mut f.pw, PW_MAX),
            3 => (&mut f.cur_pw, PW_MAX),
            4 => (&mut f.new_pw, PW_MAX),
            _ => (&mut f.set_pw, PW_MAX),
        }
    }

    /// A key while a Users field has the focus.
    pub(super) fn users_key(&mut self, k: u8, key: Key) {
        match key {
            Key::Esc => self.focus = Focus::None,
            Key::Backspace => {
                self.user_field(k).0.pop();
            }
            Key::Tab => {
                self.focus = Focus::User(match k {
                    0..=1 => k + 1,
                    2 => 0,
                    3 => 4,
                    4 => 3,
                    _ => 5,
                });
            }
            Key::Enter => {
                self.msg = None;
                match k {
                    0..=2 => {
                        self.users_create();
                    }
                    3 | 4 => {
                        self.users_save_mine();
                    }
                    _ => {
                        self.users_save_other();
                    }
                }
            }
            Key::Char(b) if (0x20..0x7F).contains(&b) => {
                // Names have no spaces; passwords and full names may.
                if b == b' ' && k == 0 {
                    return;
                }
                let (s, max) = self.user_field(k);
                if s.len() < max {
                    s.push(b as char);
                }
            }
            _ => {}
        }
    }

    fn users_create(&mut self) -> bool {
        let f = &self.users;
        let r = accounts::add_user(&f.name, &f.full, &f.pw, f.admin);
        let done = self.users_result(r, tk!("settings.users.ok.added"));
        if done {
            self.users = UsersForm::default();
            self.focus = Focus::None;
        }
        done
    }

    fn users_save_mine(&mut self) -> bool {
        let Some(me) = accounts::current() else {
            return false;
        };
        let r = accounts::set_password(&me.name, &self.users.cur_pw, &self.users.new_pw);
        let done = self.users_result(r, tk!("settings.users.ok.password"));
        if done {
            self.users.mine_open = false;
            self.users.cur_pw.clear();
            self.users.new_pw.clear();
            self.focus = Focus::None;
        }
        done
    }

    fn users_save_other(&mut self) -> bool {
        let Some(who) = self.users.set_for.clone() else {
            return false;
        };
        let r = accounts::set_password(&who, "", &self.users.set_pw);
        let done = self.users_result(r, tk!("settings.users.ok.password"));
        if done {
            self.users.set_for = None;
            self.users.set_pw.clear();
            self.focus = Focus::None;
        }
        done
    }

    /// Name of the `i`-th listed account.
    fn user_at(i: u32) -> Option<alloc::string::String> {
        accounts::users().get(i as usize).map(|u| u.name.clone())
    }

    /// A click on a Users control `hid`. `true` when it was one of ours.
    pub(super) fn users_click(&mut self, hid: u32) -> bool {
        match hid {
            h if (A_UFIELD..A_UFIELD + 6).contains(&h) => {
                self.focus = Focus::User((h - A_UFIELD) as u8);
            }
            A_UADMIN => self.users.admin = !self.users.admin,
            A_UCREATE => {
                self.users_create();
            }
            A_UMINE => {
                self.users.mine_open = true;
                // Without a password there is no "current" field: start in the new one.
                let has_pw = accounts::current().is_some_and(|me| {
                    accounts::users()
                        .iter()
                        .any(|u| u.name == me.name && u.has_password)
                });
                self.focus = Focus::User(if has_pw { 3 } else { 4 });
            }
            A_UMINE_SAVE => {
                self.users_save_mine();
            }
            A_UMINE_CANCEL => {
                self.users.mine_open = false;
                self.users.cur_pw.clear();
                self.users.new_pw.clear();
            }
            h if (A_USET..A_USET + 32).contains(&h) => {
                self.users.set_for = Self::user_at(h - A_USET);
                self.users.set_pw.clear();
                self.users.confirm_del = None;
                self.focus = Focus::User(5);
            }
            h if (A_UDEL..A_UDEL + 32).contains(&h) => {
                self.users.confirm_del = Self::user_at(h - A_UDEL);
                self.users.set_for = None;
            }
            h if (A_UROLE..A_UROLE + 32).contains(&h) => {
                if let Some(u) = accounts::users().get((h - A_UROLE) as usize) {
                    let r = accounts::set_admin(&u.name, !u.admin);
                    self.users_result(r, tk!("settings.users.ok.role"));
                }
            }
            A_USET_SAVE => {
                self.users_save_other();
            }
            A_USET_CANCEL => {
                self.users.set_for = None;
                self.users.set_pw.clear();
            }
            A_UDEL_YES => {
                if let Some(n) = self.users.confirm_del.take() {
                    let r = accounts::remove_user(&n, true);
                    self.users_result(r, tk!("settings.users.ok.removed"));
                }
            }
            A_UDEL_NO => self.users.confirm_del = None,
            _ => return false,
        }
        true
    }
}
