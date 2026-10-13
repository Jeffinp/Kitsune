//! The Som page: the sound device, the system volume, mute and a test beep (see
//! `docs/design/audio.md`).

use super::index::kv_row;
use crate::desktop::apps::ajustes::builder::Ui;
use crate::desktop::apps::ajustes::state::{A_MUTE, A_SNDTEST, A_VOLUME, ROW};
use crate::desktop::kit::ui::ButtonKind;
use crate::desktop::*;
use kitsune_core::t;

pub(super) fn page_sound(ui: &mut Ui<'_, '_>) {
    let s = ui.s;
    ui.title(t!("settings.sec.sound"));
    // What the mixer has now (the `volume` command changes it too); the stored value without a device.
    let live = crate::audio::master();
    let present = live.is_some();
    ui.header(t!("settings.sound.output"));
    let card = ui.card(ROW);
    kv_row(
        ui,
        card,
        0,
        t!("settings.sound.device"),
        if present {
            t!("settings.sound.ac97")
        } else {
            t!("settings.sound.none")
        },
    );
    if !present {
        ui.header(t!("settings.sound.none_sub"));
    }
    let card = ui.card(3 * ROW);
    let v = live.map_or(s.volume, |(v, _)| v);
    let text = alloc::format!("{v}%");
    ui.row_slider(
        card,
        0,
        t!("settings.sound.volume"),
        i32::from(v),
        (0, 100),
        &text,
        present,
        A_VOLUME,
    );
    let k = ui.st.knobs[4].value();
    ui.row_switch(
        card,
        1,
        t!("settings.sound.mute"),
        t!("settings.sound.mute_sub"),
        k,
        A_MUTE,
    );
    let r = ui.row(card, 2);
    ui.label(r, t!("settings.sound.test_label"), "", present);
    ui.button(
        Rect::new(r.right() - 16 - 120, r.y + 10, 120, 28),
        t!("settings.sound.test"),
        ButtonKind::Secondary,
        A_SNDTEST,
        present,
    );
}
