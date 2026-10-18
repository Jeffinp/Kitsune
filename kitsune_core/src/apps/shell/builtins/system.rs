//! system (split out of `builtins.rs`).

use super::*;

pub(super) fn pad2(n: u8) -> String {
    format!("{n:02}")
}

pub(super) fn date(cx: &mut CmdCtx<'_>) -> i32 {
    let t = cx.sys.now();
    let fmt = match cx.args.get(1) {
        None => "%Y-%m-%d %H:%M:%S".to_string(),
        Some(f) => match f.strip_prefix('+') {
            Some(r) => r.to_string(),
            None => {
                cx.error(&t!("sh.date.invalid", f = f.as_str()));
                return 1;
            }
        },
    };
    let mut out = String::new();
    let mut it = fmt.chars();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('Y') => out.push_str(&format!("{:04}", t.year)),
            Some('y') => out.push_str(&format!("{:02}", t.year % 100)),
            Some('m') => out.push_str(&pad2(t.month)),
            Some('d') => out.push_str(&pad2(t.day)),
            Some('H') => out.push_str(&pad2(t.hour)),
            Some('M') => out.push_str(&pad2(t.minute)),
            Some('S') => out.push_str(&pad2(t.second)),
            Some('F') => out.push_str(&format!("{:04}-{:02}-{:02}", t.year, t.month, t.day)),
            Some('T') => out.push_str(&format!("{:02}:{:02}:{:02}", t.hour, t.minute, t.second)),
            Some('%') => out.push('%'),
            Some(o) => {
                out.push('%');
                out.push(o);
            }
            None => out.push('%'),
        }
    }
    cx.println(&out);
    0
}

pub(super) fn uptime(cx: &mut CmdCtx<'_>) -> i32 {
    let total = cx.sys.uptime_ms() / 1000;
    let (d, h, m, s) = (
        total / 86_400,
        (total / 3600) % 24,
        (total / 60) % 60,
        total % 60,
    );
    let (h, m, s) = (Arg::Pad(h, 2), Arg::Pad(m, 2), Arg::Pad(s, 2));
    let line = if d > 0 {
        tp!("sh.uptime.days", d, h = h, m = m, s = s)
    } else {
        t!("sh.uptime.short", h = h, m = m, s = s)
    };
    cx.println(&line);
    0
}

pub(super) fn free(cx: &mut CmdCtx<'_>) -> i32 {
    let Some(o) = parse_opts(cx, "bkm", "", None) else {
        return 2;
    };
    let (div, unit) = if o.has('b') {
        (1u64, t!("sh.free.bytes"))
    } else if o.has('m') {
        (1024 * 1024, "MiB")
    } else {
        (1024, "KiB")
    };
    let m = cx.sys.mem();
    cx.println(&format!(
        "{:<6}{:>12}{:>12}{:>12}",
        unit,
        t!("sh.free.total"),
        t!("sh.free.used"),
        t!("sh.free.free")
    ));
    cx.println(&format!(
        "{:<6}{:>12}{:>12}{:>12}",
        "Mem:",
        m.total / div,
        m.used / div,
        m.free() / div
    ));
    0
}

pub(super) fn df(cx: &mut CmdCtx<'_>) -> i32 {
    let mut disks = cx.sys.disks();
    if disks.is_empty() {
        let u = cx.fs.usage();
        disks.push(super::super::sys::DiskInfo {
            name: String::from("rootfs"),
            mount: String::from("/"),
            total: u.total_bytes,
            used: u.used_bytes,
        });
    }
    cx.println(&format!(
        "{:<12}{:>10}{:>10}{:>10}{:>6}  {}",
        t!("sh.df.fs"),
        "KiB",
        t!("sh.df.used"),
        t!("sh.df.avail"),
        t!("sh.df.use"),
        t!("sh.df.mount")
    ));
    for d in disks {
        let pct = match d.used.saturating_mul(100).checked_div(d.total) {
            Some(p) => format!("{p}%"),
            None => String::from("-"),
        };
        cx.println(&format!(
            "{:<12}{:>10}{:>10}{:>10}{:>6}  {}",
            d.name,
            d.total / 1024,
            d.used / 1024,
            d.total.saturating_sub(d.used) / 1024,
            pct,
            d.mount
        ));
    }
    0
}

pub(super) fn ps(cx: &mut CmdCtx<'_>) -> i32 {
    cx.println(&format!(
        "{:>5}  {:<8}{:>8}  {}",
        "PID",
        t!("sh.ps.state"),
        t!("sh.ps.mem"),
        t!("sh.ps.name")
    ));
    for p in cx.sys.procs() {
        cx.println(&format!(
            "{:>5}  {:<8}{:>8}  {}",
            p.pid,
            p.state,
            p.mem_bytes / 1024,
            p.name
        ));
    }
    0
}

pub(super) fn signal_number(s: &str) -> Option<i32> {
    if let Ok(n) = s.parse::<i32>() {
        return (0..=64).contains(&n).then_some(n);
    }
    let up = s.to_ascii_uppercase();
    let name = up.strip_prefix("SIG").unwrap_or(&up);
    Some(match name {
        "HUP" => 1,
        "INT" => 2,
        "QUIT" => 3,
        "KILL" => 9,
        "TERM" => 15,
        "CONT" => 18,
        "STOP" => 19,
        _ => return None,
    })
}

pub(super) fn kill(cx: &mut CmdCtx<'_>) -> i32 {
    let mut sig = 15;
    let mut pids: Vec<String> = Vec::new();
    for a in &cx.args[1..] {
        if let Some(s) = a.strip_prefix('-')
            && pids.is_empty()
            && !s.is_empty()
        {
            match signal_number(s) {
                Some(n) => sig = n,
                None => {
                    cx.error(&t!("sh.kill.bad_signal", s = s));
                    return 2;
                }
            }
        } else {
            pids.push(a.clone());
        }
    }
    if pids.is_empty() {
        cx.error(t!("sh.kill.usage"));
        return 2;
    }
    let mut status = 0;
    for p in pids {
        match p.parse::<u32>() {
            Ok(pid) => {
                if let Err(e) = cx.sys.kill(pid, sig) {
                    cx.error(&format!("({pid}) - {}", e.message()));
                    status = 1;
                }
            }
            Err(_) => {
                cx.error(&t!("sh.kill.bad_pid", p = p.as_str()));
                status = 1;
            }
        }
    }
    status
}

/// `crash SERVICE`: for administrators, make a supervised system service fail so it is seen coming back.
pub(super) fn crash(cx: &mut CmdCtx<'_>) -> i32 {
    if cx.args.len() != 2 {
        cx.error(t!("sh.crash.usage"));
        return 2;
    }
    if cx.sys.identity().is_some_and(|who| !who.admin) {
        cx.error(t!("sh.crash.admin_only"));
        return 1;
    }
    let name = cx.args[1].clone();
    match cx.sys.crash_service(&name) {
        Ok(()) => {
            cx.println(&t!("sh.crash.sent", name = name.as_str()));
            0
        }
        Err(e) => {
            cx.error(&format!("{name}: {}", e.message()));
            1
        }
    }
}

pub(super) fn ping(cx: &mut CmdCtx<'_>) -> i32 {
    let Some(o) = parse_opts(cx, "", "c", None) else {
        return 2;
    };
    let count = match o.value('c') {
        None => 4,
        Some(v) => match v.parse::<u32>() {
            Ok(n) if (1..=100).contains(&n) => n,
            _ => {
                cx.error(&t!("sh.ping.bad_count", v = v));
                return 2;
            }
        },
    };
    let Some(host) = o.operands.first().cloned() else {
        cx.error(t!("sh.ping.usage"));
        return 2;
    };
    cx.println(&format!("PING {host}"));
    match cx.sys.ping(&host, count) {
        Ok(s) => {
            let loss = ((s.sent - s.received.min(s.sent)) * 100)
                .checked_div(s.sent)
                .unwrap_or(0);
            let line = tp!("sh.ping.stats", s.sent, recv = s.received, loss = loss);
            cx.println(&line);
            if s.received > 0 {
                let ms = dec(i64::try_from(s.avg_rtt_us).unwrap_or(i64::MAX), 3);
                cx.println(&t!("sh.ping.rtt", ms = ms));
                0
            } else {
                1
            }
        }
        Err(e) => {
            cx.error(&format!("{host}: {}", e.message()));
            1
        }
    }
}

pub(super) fn true_cmd(_: &mut CmdCtx<'_>) -> i32 {
    0
}

pub(super) fn false_cmd(_: &mut CmdCtx<'_>) -> i32 {
    1
}
