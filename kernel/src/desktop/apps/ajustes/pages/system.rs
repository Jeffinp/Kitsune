//! Network, disk, power and about pages.

use super::index::kv_row;
use crate::desktop::apps::ajustes::builder::Ui;
use crate::desktop::apps::ajustes::state::A_ADBLOCK;
use crate::desktop::apps::ajustes::state::A_REBOOT;
use crate::desktop::apps::ajustes::state::A_SHUTDOWN;
use crate::desktop::apps::ajustes::state::ROW;
use crate::desktop::kit;
use crate::desktop::kit::ui::ButtonKind;
use crate::desktop::*;
use crate::text::{self, BODY, CALLOUT, FOOTNOTE, TITLE1, TITLE2, Weight};
use kitsune_core::activity::{self};
use kitsune_core::i18n::{self, Arg};
use kitsune_core::sysif::DiskUsage;
use kitsune_core::t;

pub(super) fn page_network(ui: &mut Ui<'_, '_>, d: &Desktop) {
    use kitsune_core::netstats::NicKind;
    ui.title(t!("settings.sec.network"));
    let snap = crate::netd::stats();
    let has_nic = snap.nic != NicKind::None;
    let (word, col) = if !has_nic {
        (t!("settings.net.no_nic"), kit::ink3())
    } else if !snap.link_up {
        (t!("settings.net.no_link"), kit::red())
    } else if snap.config.is_none() {
        (t!("settings.net.waiting"), kit::amber())
    } else {
        (t!("settings.net.connected"), kit::green())
    };
    ui.header(t!("settings.net.connection"));
    let card = ui.card(ROW);
    let r = ui.row(card, 0);
    if let Some(c) = ui.c.as_deref_mut()
        && ui.view.intersection(&r).is_some()
    {
        kit::chip(c, r.x + 16, r.y + (ROW - 22) / 2, 22, word, col);
        if has_nic {
            text::draw_right(
                c,
                Rect::new(r.x, r.y, r.w - 16, r.h),
                snap.nic.name(),
                BODY,
                Weight::Regular,
                kit::ink2(),
            );
        }
    }
    ui.header(t!("settings.net.addresses"));
    let card = ui.card(5 * ROW);
    let m = crate::nic::mac();
    let mac = alloc::format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        m[0],
        m[1],
        m[2],
        m[3],
        m[4],
        m[5]
    );
    let none = String::from("—");
    let (mut ip, mut mask, mut gw, mut dns, mut lease) = (
        none.clone(),
        none.clone(),
        none.clone(),
        none.clone(),
        none.clone(),
    );
    if let Some(cfg) = snap.config {
        ip = alloc::format!("{}", cfg.ip);
        let mm = if cfg.prefix == 0 {
            0
        } else {
            u32::MAX << (32 - cfg.prefix as u32)
        };
        mask = alloc::format!(
            "{}.{}.{}.{}",
            mm >> 24,
            (mm >> 16) & 255,
            (mm >> 8) & 255,
            mm & 255
        );
        if let Some(g) = cfg.gateway {
            gw = alloc::format!("{g}");
        }
        if !cfg.dns.is_empty() {
            dns.clear();
            for (i, x) in cfg.dns.as_slice().iter().enumerate() {
                if i > 0 {
                    dns.push_str(", ");
                }
                dns.push_str(&alloc::format!("{x}"));
            }
        }
        lease = if cfg == kitsune_core::net::NetConfig::STATIC_FALLBACK {
            String::from(t!("settings.net.static"))
        } else if let Some(ms) = snap.lease_remaining_ms {
            t!(
                "settings.net.left",
                t = kit::fb_str(&activity::fmt_elapsed(ms / 1000))
            )
        } else {
            String::from(t!("settings.net.no_expiry"))
        };
    }
    kv_row(ui, card, 0, t!("settings.net.ip"), &ip);
    kv_row(ui, card, 1, t!("settings.net.mask"), &mask);
    kv_row(ui, card, 2, t!("settings.net.router"), &gw);
    kv_row(ui, card, 3, "DNS", &dns);
    kv_row(ui, card, 4, t!("settings.net.lease"), &lease);
    ui.header(t!("settings.net.device"));
    let card = ui.card(4 * ROW);
    kv_row(
        ui,
        card,
        0,
        t!("settings.net.mac"),
        if has_nic { &mac } else { "—" },
    );
    let rx = alloc::format!(
        "{} · {}/s",
        i18n::format_size(snap.rx_bytes),
        i18n::format_size(d.sysmon.rx_rate)
    );
    let tx = alloc::format!(
        "{} · {}/s",
        i18n::format_size(snap.tx_bytes),
        i18n::format_size(d.sysmon.tx_rate)
    );
    kv_row(ui, card, 1, t!("settings.net.received"), &rx);
    kv_row(ui, card, 2, t!("settings.net.sent"), &tx);
    let pk = t!(
        "settings.net.packets_value",
        rx = Arg::Num(snap.rx_packets as i64),
        tx = Arg::Num(snap.tx_packets as i64)
    );
    kv_row(ui, card, 3, t!("settings.net.packets"), &pk);
    ui.header(t!("settings.net.privacy"));
    let card = ui.card(ROW);
    let k = ui.st.knobs[3].value();
    ui.row_switch(
        card,
        0,
        t!("settings.net.adblock"),
        t!("settings.net.adblock_sub"),
        k,
        A_ADBLOCK,
    );
}

pub(super) fn page_disk(ui: &mut Ui<'_, '_>, d: &Desktop) {
    ui.title(t!("settings.sec.disk"));
    let usage = VfsUsage;
    let u = vfs::statfs();
    let (title, sub) = match vfs::volume() {
        vfs::Volume::Disk => (t!("settings.disk.main"), t!("settings.disk.main_sub")),
        vfs::Volume::Memory => (t!("settings.disk.memory"), t!("settings.disk.memory_sub")),
    };
    let _ = usage.label();
    ui.header(t!("settings.disk.volume"));
    let card = ui.card(104);
    if let Some(c) = ui.c.as_deref_mut()
        && ui.view.intersection(&card).is_some()
    {
        text::draw_left(
            c,
            Rect::new(card.x + 20, card.y + 14, card.w / 2, 24),
            title,
            CALLOUT,
            Weight::Semibold,
            kit::ink(),
        );
        text::draw_left(
            c,
            Rect::new(card.x + 20, card.y + 38, card.w - 140, 18),
            sub,
            FOOTNOTE,
            Weight::Regular,
            kit::ink2(),
        );
        let pm = u.used_permille();
        let pct = t!("settings.pct", p = i18n::dec(pm as i64, 1));
        text::draw_right(
            c,
            Rect::new(card.x, card.y + 14, card.w - 20, 28),
            &pct,
            TITLE2,
            Weight::Semibold,
            theme::accent(),
        );
        kit::bar(
            c,
            Rect::new(card.x + 20, card.y + 66, card.w - 40, 12),
            (pm as i64) << 8,
            if pm >= 900 {
                kit::red()
            } else {
                theme::accent()
            },
        );
    }
    let card = ui.card(4 * ROW);
    kv_row(
        ui,
        card,
        0,
        t!("settings.disk.used"),
        &i18n::format_size(u.used()),
    );
    kv_row(
        ui,
        card,
        1,
        t!("settings.disk.free"),
        &i18n::format_size(u.free),
    );
    kv_row(
        ui,
        card,
        2,
        t!("settings.disk.total"),
        &i18n::format_size(u.total),
    );
    let files = if u.inodes_total > 0 {
        i18n::format_num(u.inodes_used() as i64)
    } else {
        String::from("—")
    };
    kv_row(ui, card, 3, t!("settings.disk.items"), &files);
    ui.header(t!("settings.disk.devices"));
    let card = ui.card(2 * ROW);
    for (i, label) in [t!("settings.disk.boot_disk"), t!("settings.disk.data_disk")]
        .iter()
        .enumerate()
    {
        let v = match d.disks.get(i).copied().flatten() {
            Some(dk) => alloc::format!(
                "{} · {}",
                dk.model_name(),
                i18n::format_size(dk.mib() << 20)
            ),
            None => String::from(t!("settings.disk.absent")),
        };
        kv_row(ui, card, i as i32, label, &v);
    }
}

pub(super) fn page_power(ui: &mut Ui<'_, '_>) {
    ui.title(t!("settings.sec.power"));
    let card = ui.card(2 * ROW);
    for (i, (name, label, id)) in [
        (t!("power.restart"), t!("menu.system.restart"), A_REBOOT),
        (t!("power.shutdown"), t!("menu.system.shutdown"), A_SHUTDOWN),
    ]
    .into_iter()
    .enumerate()
    {
        let r = ui.row(card, i as i32);
        ui.label(r, name, "", true);
        ui.button(
            Rect::new(r.right() - 16 - 120, r.y + 10, 120, 28),
            label,
            if i == 1 {
                ButtonKind::Destructive
            } else {
                ButtonKind::Secondary
            },
            id,
            true,
        );
    }
}

pub(super) fn page_about(ui: &mut Ui<'_, '_>, d: &Desktop) {
    ui.title(t!("settings.sec.about"));
    let head = Rect::new(ui.x, ui.y, ui.w, 88);
    if let Some(c) = ui.c.as_deref_mut()
        && ui.view.intersection(&head).is_some()
    {
        // The fox with its tails: a wide mark, so it gets a wider box than the old square icon.
        icons::blit(c, Icon::Halo, head.x - 6, head.y - 4, 96, 256);
        text::draw_left(
            c,
            Rect::new(head.x + 100, head.y + 6, head.w - 100, 32),
            "Kitsune",
            TITLE1,
            Weight::Semibold,
            kit::ink(),
        );
        let v = t!(
            "settings.about.version",
            v = env!("CARGO_PKG_VERSION"),
            build = if cfg!(debug_assertions) {
                t!("settings.about.build_debug")
            } else {
                t!("settings.about.build_release")
            }
        );
        text::draw_left(
            c,
            Rect::new(head.x + 100, head.y + 42, head.w - 100, 22),
            &v,
            BODY,
            Weight::Regular,
            kit::ink2(),
        );
    }
    ui.y += 104;
    let card = ui.card(6 * ROW);
    let si = crate::sysinfo::get();
    let cpu = si
        .map(|s| {
            let b = core::str::from_utf8(s.brand.as_bytes())
                .unwrap_or("")
                .trim();
            if b.is_empty() {
                String::from(core::str::from_utf8(s.vendor.as_bytes()).unwrap_or("—"))
            } else {
                String::from(b)
            }
        })
        .unwrap_or_else(|| String::from("—"));
    kv_row(ui, card, 0, t!("settings.about.cpu"), &cpu);
    let ram = si.map_or(String::from("—"), |s| i18n::format_size(s.total_ram()));
    kv_row(ui, card, 1, t!("settings.about.memory"), &ram);
    let up = alloc::format!("{}", activity::fmt_clock(d.sysmon.uptime_s));
    kv_row(ui, card, 2, t!("settings.about.uptime"), &up);
    let res = si.map_or(String::from("—"), |s| {
        t!(
            "settings.about.resolution",
            w = Arg::Int(s.width as i64),
            h = Arg::Int(s.height as i64)
        )
    });
    kv_row(ui, card, 3, t!("settings.about.display"), &res);
    let boot = si.map_or(String::from("—"), |s| {
        if s.hypervisor.as_bytes().is_empty() {
            String::from(s.boot_mode)
        } else {
            t!("settings.about.vm", mode = s.boot_mode)
        }
    });
    kv_row(ui, card, 4, t!("settings.about.boot"), &boot);
    let sec = match crate::rng::quality() {
        kitsune_core::entropy::Quality::Strong => t!("settings.about.rng_strong"),
        kitsune_core::entropy::Quality::Mixed => t!("settings.about.rng_mixed"),
        kitsune_core::entropy::Quality::Weak => t!("settings.about.rng_weak"),
    };
    kv_row(ui, card, 5, t!("settings.about.security"), sec);
}
