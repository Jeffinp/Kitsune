//! The page dispatcher and the helpers pages share.

use super::look::page_appearance;
use super::look::page_dock;
use super::look::page_wallpaper;
use super::region::page_keyboard;
use super::region::page_language;
use super::region::page_time;
use super::sound::page_sound;
use super::system::page_about;
use super::system::page_disk;
use super::system::page_network;
use super::system::page_power;
use super::users::page_users;
use crate::desktop::apps::ajustes::builder::Ui;
use crate::desktop::apps::ajustes::state::*;
use crate::desktop::kit;
use crate::desktop::*;

// ------------------------------------------------------------------ pages

pub(in super::super) fn page(ui: &mut Ui<'_, '_>, d: &Desktop) {
    match ui.st.section {
        0 => page_appearance(ui),
        1 => page_wallpaper(ui),
        2 => page_dock(ui),
        3 => page_keyboard(ui),
        4 => page_time(ui),
        S_LANG => page_language(ui),
        6 => page_network(ui, d),
        7 => page_disk(ui, d),
        8 => page_power(ui),
        S_USERS => page_users(ui),
        S_SOUND => page_sound(ui),
        _ => page_about(ui, d),
    }
    ui.st
        .content_h
        .set(ui.y + ui.st.scroll.value() - ui.view.y + 8);
}

/// One `name .... value` row of a read-only card.
pub(super) fn kv_row(ui: &mut Ui<'_, '_>, card: Rect, i: i32, name: &str, value: &str) {
    let r = ui.row(card, i);
    if let Some(c) = ui.c.as_deref_mut()
        && ui.view.intersection(&r).is_some()
    {
        kit::kv(c, Rect::new(r.x + 16, r.y, r.w - 32, r.h), name, value);
    }
}
