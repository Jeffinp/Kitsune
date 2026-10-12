//! The time-zone picker: cities with their standard offset, names, search and the offset label.

use super::*;

/// `Settings::tz_city` value for "none of the listed cities".
pub const CITY_NONE: u8 = 255;

/// Cities of the time-zone picker with their standard offset in minutes east of UTC,
/// ordered by offset. The settings store the offset (and the index, to show the city the
/// user chose); there is no daylight saving.
pub const TIMEZONES: [(&str, i16); 52] = [
    (tk!("settings.tz.baker_island"), -720),
    (tk!("settings.tz.pago_pago"), -660),
    (tk!("settings.tz.honolulu"), -600),
    (tk!("settings.tz.anchorage"), -540),
    (tk!("settings.tz.los_angeles"), -480),
    (tk!("settings.tz.denver"), -420),
    (tk!("settings.tz.mexico_city"), -360),
    (tk!("settings.tz.chicago"), -360),
    (tk!("settings.tz.new_york"), -300),
    (tk!("settings.tz.bogota"), -300),
    (tk!("settings.tz.rio_branco"), -300),
    (tk!("settings.tz.manaus"), -240),
    (tk!("settings.tz.santiago"), -240),
    (tk!("settings.tz.caracas"), -240),
    (tk!("settings.tz.st_johns"), -210),
    (tk!("settings.tz.buenos_aires"), -180),
    (tk!("settings.tz.sao_paulo"), -180),
    (tk!("settings.tz.brasilia"), -180),
    (tk!("settings.tz.noronha"), -120),
    (tk!("settings.tz.azores"), -60),
    (tk!("settings.tz.lisbon"), 0),
    (tk!("settings.tz.london"), 0),
    (tk!("settings.tz.reykjavik"), 0),
    (tk!("settings.tz.madrid"), 60),
    (tk!("settings.tz.paris"), 60),
    (tk!("settings.tz.berlin"), 60),
    (tk!("settings.tz.rome"), 60),
    (tk!("settings.tz.athens"), 120),
    (tk!("settings.tz.cairo"), 120),
    (tk!("settings.tz.johannesburg"), 120),
    (tk!("settings.tz.moscow"), 180),
    (tk!("settings.tz.istanbul"), 180),
    (tk!("settings.tz.nairobi"), 180),
    (tk!("settings.tz.tehran"), 210),
    (tk!("settings.tz.dubai"), 240),
    (tk!("settings.tz.kabul"), 270),
    (tk!("settings.tz.karachi"), 300),
    (tk!("settings.tz.new_delhi"), 330),
    (tk!("settings.tz.kathmandu"), 345),
    (tk!("settings.tz.dhaka"), 360),
    (tk!("settings.tz.bangkok"), 420),
    (tk!("settings.tz.beijing"), 480),
    (tk!("settings.tz.singapore"), 480),
    (tk!("settings.tz.tokyo"), 540),
    (tk!("settings.tz.seoul"), 540),
    (tk!("settings.tz.adelaide"), 570),
    (tk!("settings.tz.sydney"), 600),
    (tk!("settings.tz.solomon"), 660),
    (tk!("settings.tz.auckland"), 720),
    (tk!("settings.tz.chatham"), 765),
    (tk!("settings.tz.nukualofa"), 780),
    (tk!("settings.tz.kiritimati"), 840),
];

/// The name of city `i` of [`TIMEZONES`] in `lang` (empty for an index out of range).
pub fn city_name_in(lang: Lang, i: u8) -> &'static str {
    TIMEZONES
        .get(i as usize)
        .map_or("", |&(key, _)| crate::i18n::tr_in(lang, key))
}

/// The name of city `i` in the language in effect.
pub fn city_name(i: u8) -> &'static str {
    city_name_in(crate::i18n::lang(), i)
}

/// The index of Brasília, the default city.
pub const DEFAULT_CITY: u8 = 17;

/// `UTC-03:00` / `UTC+05:30` for an offset in minutes.
pub fn utc_label(minutes: i32) -> FixedBuf<12> {
    let mut b = FixedBuf::new();
    let _ = write!(
        b,
        "UTC{}{:02}:{:02}",
        if minutes < 0 { '-' } else { '+' },
        minutes.abs() / 60,
        minutes.abs() % 60
    );
    b
}

/// The first listed city at `minutes` (Brasília for -180), or [`CITY_NONE`].
pub fn city_for_offset(minutes: i32) -> u8 {
    if minutes == -180 {
        return DEFAULT_CITY;
    }
    TIMEZONES
        .iter()
        .position(|&(_, m)| m as i32 == minutes)
        .map_or(CITY_NONE, |i| i as u8)
}

/// Indexes into [`TIMEZONES`] whose city (in any language: `Lisboa` and `Lisbon` find the
/// same row) or `UTC+hh:mm` label contains `query` (accents and case ignored), in list
/// order. An empty query lists every city.
pub fn search_timezones(query: &str) -> Vec<u8> {
    let q = crate::format::search::fold(query.trim());
    TIMEZONES
        .iter()
        .enumerate()
        .filter(|(_, (key, m))| {
            q.is_empty()
                || Lang::ALL.iter().any(|&l| {
                    crate::format::search::fold(crate::i18n::tr_in(l, key)).contains(q.as_str())
                })
                || crate::format::search::fold(
                    core::str::from_utf8(utc_label(*m as i32).as_bytes()).unwrap_or(""),
                )
                .contains(q.as_str())
        })
        .map(|(i, _)| i as u8)
        .collect()
}
