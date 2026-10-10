//! The Users page: your account, everybody's accounts and a form to add one.

use crate::desktop::apps::ajustes::builder::Ui;
use crate::desktop::apps::ajustes::state::*;
use crate::desktop::kit;
use crate::desktop::kit::ui::ButtonKind;
use crate::desktop::services::accounts;
use crate::desktop::*;
use crate::text::{self, BODY, FOOTNOTE, Weight};
use kitsune_core::secret::Secret;
use kitsune_core::t;

/// Dots standing for a typed password.
fn mask(s: &Secret) -> String {
    "•".repeat(s.chars())
}

fn role(admin: bool) -> &'static str {
    if admin {
        t!("login.admin")
    } else {
        t!("login.user")
    }
}

/// A line of status text (the last result) at `r`.
fn say(ui: &mut Ui<'_, '_>, r: Rect) {
    if let Some((m, err)) = ui.st.message()
        && let Some(c) = ui.c.as_deref_mut()
        && ui.view.intersection(&r).is_some()
    {
        text::draw_left(
            c,
            r,
            &m,
            FOOTNOTE,
            Weight::Regular,
            if err { kit::red() } else { kit::ink2() },
        );
    }
}

pub(in super::super) fn page_users(ui: &mut Ui<'_, '_>) {
    ui.title(t!("settings.sec.users"));
    let me = accounts::current();
    let admin = me.as_ref().is_some_and(|s| s.admin);
    mine(ui, me.as_ref());
    auto_lock(ui);
    list(ui, me.as_ref().map(|s| s.name.as_str()), admin);
    if admin {
        add_form(ui);
    } else {
        say(ui, Rect::new(ui.x + 4, ui.y, ui.w - 8, 20));
        ui.y += 26;
    }
}

fn mine(ui: &mut Ui<'_, '_>, me: Option<&accounts::Session>) {
    let Some(me) = me else {
        return;
    };
    ui.header(t!("settings.users.mine"));
    let open = ui.st.users.mine_open;
    // Without a password there is no current one to ask for.
    let has_pw = accounts::users()
        .iter()
        .any(|u| u.name == me.name && u.has_password);
    let card = ui.card(
        ROW * if !open {
            1
        } else if has_pw {
            4
        } else {
            3
        },
    );
    let r = ui.row(card, 0);
    let name = if me.full_name.is_empty() {
        me.name.as_str()
    } else {
        me.full_name.as_str()
    };
    let sub = alloc::format!("{} · {}", me.name, role(me.admin));
    ui.label(r, name, &sub, true);
    if !open {
        ui.button(
            Rect::new(r.right() - 16 - 128, r.y + 10, 128, 28),
            t!("settings.users.change_pw"),
            ButtonKind::Secondary,
            A_UMINE,
            true,
        );
        return;
    }
    let f = &ui.st.users;
    let (cur, new) = (mask(&f.cur_pw), mask(&f.new_pw));
    let (fc, fnew) = (ui.st.focus == Focus::User(3), ui.st.focus == Focus::User(4));
    let mut fields = alloc::vec::Vec::new();
    if has_pw {
        fields.push((t!("settings.users.cur_pw"), cur, fc, A_UFIELD + 3));
    }
    fields.push((t!("settings.users.new_pw"), new, fnew, A_UFIELD + 4));
    let n_fields = fields.len() as i32;
    for (i, (label, val, foc, id)) in fields.into_iter().enumerate() {
        let r = ui.row(card, 1 + i as i32);
        ui.label(r, label, "", true);
        let fr = Rect::new(r.x + 200, r.y + 10, r.w - 216, 28);
        ui.field(fr, &val, t!("settings.users.pw_hint"), foc, id);
    }
    let r = ui.row(card, 1 + n_fields);
    say(ui, Rect::new(r.x + 16, r.y, r.w - 200, r.h));
    ui.button(
        Rect::new(r.right() - 16 - 88, r.y + 10, 88, 28),
        t!("settings.users.save"),
        ButtonKind::Primary,
        A_UMINE_SAVE,
        true,
    );
    ui.button(
        Rect::new(r.right() - 16 - 88 - 8 - 88, r.y + 10, 88, 28),
        t!("settings.users.cancel"),
        ButtonKind::Secondary,
        A_UMINE_CANCEL,
        true,
    );
}

fn auto_lock(ui: &mut Ui<'_, '_>) {
    use kitsune_core::settings::AUTO_LOCK_CHOICES;
    ui.header(t!("settings.users.lock_h"));
    let card = ui.card(ROW);
    let r = ui.row(card, 0);
    ui.label(
        r,
        t!("settings.users.lock"),
        t!("settings.users.lock_sub"),
        true,
    );
    let now = ui.s.auto_lock_min;
    let sel = AUTO_LOCK_CHOICES
        .iter()
        .position(|&m| m == now)
        .unwrap_or(0);
    let labels = [
        t!("settings.users.lock_never"),
        t!("settings.users.lock_5"),
        t!("settings.users.lock_15"),
        t!("settings.users.lock_30"),
    ];
    let seg = Rect::new(r.right() - 16 - 250, r.y + 10, 250, 28);
    ui.segmented(seg, &labels, sel, A_ULOCK);
}

fn list(ui: &mut Ui<'_, '_>, me: Option<&str>, admin: bool) {
    ui.header(t!("settings.users.accounts"));
    let users = accounts::users();
    let card = ui.card(ROW * users.len().max(1) as i32);
    for (i, u) in users.iter().enumerate().take(32) {
        let r = ui.row(card, i as i32);
        let n = i as u32;
        let mut sub = alloc::format!("{} · {}", u.name, role(u.admin));
        if !u.has_password {
            sub.push_str(" · ");
            sub.push_str(t!("settings.users.no_pw"));
        }
        if me == Some(u.name.as_str()) {
            sub.push_str(" · ");
            sub.push_str(t!("settings.users.you"));
        }
        let y = r.y + 10;
        let deleting = ui.st.users.confirm_del.as_deref() == Some(u.name.as_str());
        let setting = ui.st.users.set_for.as_deref() == Some(u.name.as_str());
        let controls = if !admin {
            0
        } else if deleting {
            88 + 8 + 88
        } else if setting {
            150 + 8 + 80 + 8 + 80
        } else {
            72 + 8 + 100 + 8 + 84
        };
        if let Some(c) = ui.c.as_deref_mut()
            && ui.view.intersection(&r).is_some()
        {
            let w = (r.w - 32 - controls - 8).max(60);
            text::draw_ellipsis(
                c,
                r.x + 16,
                r.y + 8,
                w,
                accounts::display_name(u),
                BODY,
                Weight::Regular,
                kit::ink(),
            );
            text::draw_ellipsis(
                c,
                r.x + 16,
                r.y + 27,
                w,
                &sub,
                FOOTNOTE,
                Weight::Regular,
                kit::ink2(),
            );
        }
        if !admin {
            continue;
        }
        let right = r.right() - 16;
        if deleting {
            ui.button(
                Rect::new(right - 88, y, 88, 28),
                t!("settings.users.cancel"),
                ButtonKind::Secondary,
                A_UDEL_NO,
                true,
            );
            ui.button(
                Rect::new(right - 88 - 8 - 88, y, 88, 28),
                t!("settings.users.remove"),
                ButtonKind::Destructive,
                A_UDEL_YES,
                true,
            );
        } else if setting {
            ui.button(
                Rect::new(right - 80, y, 80, 28),
                t!("settings.users.cancel"),
                ButtonKind::Secondary,
                A_USET_CANCEL,
                true,
            );
            ui.button(
                Rect::new(right - 80 - 8 - 80, y, 80, 28),
                t!("settings.users.save"),
                ButtonKind::Primary,
                A_USET_SAVE,
                true,
            );
            let pw = mask(&ui.st.users.set_pw);
            let foc = ui.st.focus == Focus::User(5);
            ui.field(
                Rect::new(right - 80 - 8 - 80 - 8 - 150, y, 150, 28),
                &pw,
                t!("settings.users.pw_hint"),
                foc,
                A_UFIELD + 5,
            );
        } else {
            ui.button(
                Rect::new(right - 84, y, 84, 28),
                t!("settings.users.remove"),
                ButtonKind::Secondary,
                A_UDEL + n,
                me != Some(u.name.as_str()),
            );
            let flip = if u.admin {
                t!("settings.users.make_user")
            } else {
                t!("settings.users.make_admin")
            };
            ui.button(
                Rect::new(right - 84 - 8 - 100, y, 100, 28),
                flip,
                ButtonKind::Secondary,
                A_UROLE + n,
                me != Some(u.name.as_str()),
            );
            ui.button(
                Rect::new(right - 84 - 8 - 100 - 8 - 72, y, 72, 28),
                t!("settings.users.password"),
                ButtonKind::Secondary,
                A_USET + n,
                true,
            );
        }
    }
}

fn add_form(ui: &mut Ui<'_, '_>) {
    ui.header(t!("settings.users.new"));
    let card = ui.card(ROW * 5);
    let f = &ui.st.users;
    let vals = [f.name.clone(), f.full.clone(), mask(&f.pw)];
    let labels = [
        t!("settings.users.name"),
        t!("settings.users.full"),
        t!("settings.users.password"),
    ];
    let hints = [
        t!("settings.users.name_hint"),
        t!("settings.users.full_hint"),
        t!("settings.users.pw_opt"),
    ];
    for i in 0..3 {
        let r = ui.row(card, i as i32);
        ui.label(r, labels[i], "", true);
        let fr = Rect::new(r.x + 200, r.y + 10, r.w - 216, 28);
        let foc = ui.st.focus == Focus::User(i as u8);
        ui.field(fr, &vals[i], hints[i], foc, A_UFIELD + i as u32);
    }
    let knob = if ui.st.users.admin { 256 } else { 0 };
    ui.row_switch(
        card,
        3,
        t!("settings.users.admin"),
        t!("settings.users.admin_sub"),
        knob,
        A_UADMIN,
    );
    let r = ui.row(card, 4);
    say(ui, Rect::new(r.x + 16, r.y, r.w - 160, r.h));
    let ok = !ui.st.users.name.is_empty();
    ui.button(
        Rect::new(r.right() - 16 - 120, r.y + 10, 120, 28),
        t!("settings.users.create"),
        ButtonKind::Primary,
        A_UCREATE,
        ok,
    );
}
