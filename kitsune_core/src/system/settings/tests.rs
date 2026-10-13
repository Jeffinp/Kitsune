use super::*;

#[test]
fn defaults_are_the_new_desktop() {
    let s = Settings::default();
    assert_eq!(s.wallpaper, WallpaperChoice::Preset(0));
    assert_eq!(s.accent, 0);
    assert!(s.clock24);
    assert_eq!(s.tz_minutes, -180);
    assert_eq!(s.layout, Layout::Us);
    assert!(s.toasts);
    assert_eq!(s.accent_rgb(), 0x5B5CF6);
    assert_eq!(s.appearance, AppearanceSetting::Auto);
    assert!(!s.reduce_motion);
    assert!(s.image_path().is_empty());
}

#[test]
fn roundtrip() {
    let mut s = Settings {
        wallpaper: WallpaperChoice::Image,
        accent: 5,
        clock24: false,
        clock_auto: false,
        lang: Lang::En,
        tz_minutes: 330,
        tz_city: city_for_offset(330),
        layout: Layout::Abnt2,
        toasts: false,
        appearance: AppearanceSetting::Dark,
        reduce_motion: true,
        ..Settings::default()
    };
    assert!(s.set_image_path(b"fotos/praia-1.png"));
    let text = s.to_text();
    assert_eq!(Settings::parse(&text), s);
    // Idempotent.
    assert_eq!(Settings::parse(&text).to_text(), text);
}

#[test]
fn language_setting_parses_and_roundtrips() {
    assert_eq!(Settings::default().lang, Lang::Pt);
    for (text, want) in [
        ("language=en\n", Lang::En),
        ("language=en-US\n", Lang::En),
        ("language=pt\n", Lang::Pt),
        ("language=PT_br\n", Lang::Pt),
        ("language=fr\n", Lang::Pt),
        ("language=\n", Lang::Pt),
        ("language=en\nlanguage=klingon\n", Lang::En),
        ("language=\u{ff}\u{fe}\n", Lang::Pt),
    ] {
        assert_eq!(Settings::parse(text.as_bytes()).lang, want, "{text:?}");
    }
    let mut s = Settings::default();
    s.set_language(Lang::En);
    assert!(s.to_text().windows(11).any(|w| w == b"language=en"));
    assert_eq!(Settings::parse(&s.to_text()), s);
    s.set_language(Lang::Pt);
    assert!(s.to_text().windows(14).any(|w| w == b"language=pt-BR"));
    assert_eq!(Settings::parse(&s.to_text()), s);
}

#[test]
fn clock_follows_the_language_until_the_user_chooses() {
    let mut s = Settings::default();
    assert!(s.clock_auto && s.clock24);
    s.set_language(Lang::En);
    assert!(!s.clock24, "English defaults to 12 hours");
    s.set_language(Lang::Pt);
    assert!(s.clock24);
    // An explicit choice sticks across language changes.
    s.set_clock24(false);
    assert!(!s.clock_auto && !s.clock24);
    s.set_language(Lang::En);
    s.set_language(Lang::Pt);
    assert!(!s.clock24);
    s.follow_language_clock();
    assert!(s.clock_auto && s.clock24);
    // Text form: `auto` or the number; the order of the lines does not matter.
    assert!(
        Settings::default()
            .to_text()
            .windows(10)
            .any(|w| w == b"clock=auto")
    );
    let en = Settings::parse(b"clock=auto\nlanguage=en\n");
    assert!(en.clock_auto && !en.clock24);
    let en2 = Settings::parse(b"language=en\nclock=auto\n");
    assert_eq!(en, en2);
    let fixed = Settings::parse(b"language=en\nclock=24\n");
    assert!(!fixed.clock_auto && fixed.clock24);
    assert!(fixed.to_text().windows(8).any(|w| w == b"clock=24"));
    // Files written before the language existed keep their explicit clock.
    let old = Settings::parse(b"version=1\nclock=12\n");
    assert!(!old.clock_auto && !old.clock24 && old.lang == Lang::Pt);
    // Garbage keeps the default (auto).
    assert!(Settings::parse(b"clock=banana\n").clock_auto);
}

#[test]
fn default_roundtrips_and_keeps_text_small() {
    let s = Settings::default();
    let text = s.to_text();
    assert_eq!(Settings::parse(&text), s);
    assert!(text.len() < 240, "{}", text.len());
    assert!(text.starts_with(b"# Kitsune settings\nversion=1\n"));
}

#[test]
fn negative_timezone_roundtrip() {
    let mut s = Settings::default();
    for tz in [-720, -180, 0, 60, 330, 840] {
        s.tz_minutes = tz;
        assert_eq!(Settings::parse(&s.to_text()).tz_minutes, tz);
    }
}

#[test]
fn empty_and_garbage_give_defaults() {
    assert_eq!(Settings::parse(b""), Settings::default());
    assert_eq!(Settings::parse(b"\n\n   \n"), Settings::default());
    assert_eq!(Settings::parse(b"\xFF\xFE\x00garbage"), Settings::default());
    assert_eq!(
        Settings::parse(b"no equals here\n=\n==\n"),
        Settings::default()
    );
}

#[test]
fn invalid_values_keep_the_default() {
    let s = Settings::parse(
            b"accent=99\nclock=13\ntz=99999\nkeyboard=klingon\ntoasts=maybe\nwallpaper=77\nwallpaper_path=bad path!\n",
        );
    assert_eq!(s, Settings::default());
    let s = Settings::parse(b"accent=abc\ntz=--5\ntz=\naccent=\nwallpaper=-1\n");
    assert_eq!(s, Settings::default());
}

#[test]
fn valid_lines_survive_invalid_neighbours() {
    let s = Settings::parse(b"accent=zzz\naccent=4\nclock=24\nclock=12\ntz=bad\n");
    assert_eq!(s.accent, 4);
    assert!(!s.clock24);
    assert_eq!(s.tz_minutes, -180);
}

#[test]
fn comments_whitespace_crlf_and_unknown_keys() {
    let s = Settings::parse(
            b"# comment\r\n  accent = 2  \r\nfuture_key=whatever\nversion=99\n  # indented comment\nclock=12\r\n",
        );
    assert_eq!(s.accent, 2);
    assert!(!s.clock24);
}

#[test]
fn later_lines_win() {
    assert_eq!(Settings::parse(b"accent=1\naccent=6\n").accent, 6);
}

#[test]
fn image_wallpaper_needs_a_path() {
    // No path anywhere: default wallpaper.
    assert_eq!(
        Settings::parse(b"wallpaper=image\n").wallpaper,
        WallpaperChoice::Preset(0)
    );
    // Path before or after the choice both work.
    let a = Settings::parse(b"wallpaper=image\nwallpaper_path=a.png\n");
    let b = Settings::parse(b"wallpaper_path=a.png\nwallpaper=image\n");
    assert_eq!(a.wallpaper, WallpaperChoice::Image);
    assert_eq!(a, b);
    assert_eq!(a.image_path(), b"a.png");
    // A preset choice keeps a stored path around for later.
    let c = Settings::parse(b"wallpaper_path=a.png\nwallpaper=3\n");
    assert_eq!(c.wallpaper, WallpaperChoice::Preset(3));
    assert_eq!(c.image_path(), b"a.png");
}

#[test]
fn path_validation() {
    let mut s = Settings::default();
    assert!(!s.set_image_path(b""));
    assert!(!s.set_image_path(b"has space.png"));
    assert!(!s.set_image_path(b"../etc/passwd\n"));
    assert!(!s.set_image_path(&[b'a'; PATH_CAP + 1]));
    assert!(s.set_image_path(&[b'a'; PATH_CAP]));
    assert!(s.set_image_path(b"a/b_c-d.PNG"));
    assert_eq!(s.image_path(), b"a/b_c-d.PNG");
    // A refused path leaves the previous one.
    assert!(!s.set_image_path(b"bad path"));
    assert_eq!(s.image_path(), b"a/b_c-d.PNG");
}

#[test]
fn accent_palette_head_is_the_theme_indigo() {
    assert_eq!(ACCENTS[0], 0x5B5CF6);
    assert_eq!(ACCENTS.len(), ACCENT_NAMES.len());
    for l in Lang::ALL {
        for k in ACCENT_NAMES {
            assert!(!crate::i18n::tr_in(l, k).is_empty() && crate::i18n::tr_in(l, k) != k);
        }
    }
    assert_eq!(crate::i18n::tr_in(Lang::Pt, ACCENT_NAMES[5]), "Âmbar");
    assert_eq!(crate::i18n::tr_in(Lang::En, ACCENT_NAMES[5]), "Amber");
    let s = Settings {
        accent: 200,
        ..Settings::default()
    };
    assert_eq!(s.accent_rgb(), ACCENTS[7]);
}

#[test]
fn appearance_and_reduce_motion_parse_and_stay_total() {
    let s = Settings::parse(b"appearance=dark\nreduce_motion=1\n");
    assert_eq!(s.appearance, AppearanceSetting::Dark);
    assert!(s.reduce_motion);
    // Bad values keep the default, good neighbours survive.
    let s = Settings::parse(b"appearance=sepia\nreduce_motion=maybe\nappearance=light\n");
    assert_eq!(s.appearance, AppearanceSetting::Light);
    assert!(!s.reduce_motion);
    assert_eq!(
        Settings::parse(b"appearance=\n=dark\n").appearance,
        AppearanceSetting::Auto
    );
    // Old files without the new keys read as the defaults.
    let old = Settings::parse(b"version=1\nwallpaper=2\naccent=3\nclock=12\n");
    assert_eq!(old.appearance, AppearanceSetting::Auto);
    assert!(!old.reduce_motion);
    assert!(
        old.to_text()
            .starts_with(b"# Kitsune settings\nversion=1\nwallpaper=2\n")
    );
    assert_eq!(old.toast_secs, TOAST_SECS_DEFAULT);
    assert_eq!(old.dock_zoom, DOCK_ZOOM_MAX);
}

#[test]
fn font_sizes_parse_inside_their_range_and_round_trip() {
    let s = Settings::parse(b"terminal_font=18\neditor_font=12\n");
    assert_eq!((s.terminal_font, s.editor_font), (18, 12));
    let text = s.to_text();
    assert!(text.ends_with(b"terminal_font=18\neditor_font=12\n"));
    assert_eq!(Settings::parse(&text), s);
    // Out of range, not a number, empty: the default stays, neighbours survive.
    for bad in [
        &b"terminal_font=10\n"[..],
        b"terminal_font=25\n",
        b"terminal_font=x\n",
        b"terminal_font=\n",
        b"terminal_font=-3\n",
        b"terminal_font=99999999999\n",
    ] {
        assert_eq!(Settings::parse(bad).terminal_font, FONT_DEFAULT, "{bad:?}");
    }
    let s = Settings::parse(b"editor_font=40\nterminal_font=20\n");
    assert_eq!((s.terminal_font, s.editor_font), (20, FONT_DEFAULT));
    // The limits themselves are valid.
    let s = Settings::parse(b"terminal_font=11\neditor_font=24\n");
    assert_eq!((s.terminal_font, s.editor_font), (11, 24));
    // The default is not written, so old and new files stay small and equal.
    assert!(
        !Settings::default()
            .to_text()
            .windows(5)
            .any(|w| w == b"_font")
    );
}

#[test]
fn font_steps_stay_in_range() {
    assert_eq!(font_step(15, 1), 16);
    assert_eq!(font_step(15, -1), 14);
    assert_eq!(font_step(15, 0), FONT_DEFAULT);
    assert_eq!(font_step(FONT_MAX, 1), FONT_MAX);
    assert_eq!(font_step(FONT_MIN, -1), FONT_MIN);
    // A stored value outside the range is first pulled in.
    assert_eq!(font_step(200, -1), FONT_MAX - 1);
    assert_eq!(font_step(0, 1), FONT_MIN + 1);
    let mut px = FONT_DEFAULT;
    for _ in 0..40 {
        px = font_step(px, 1);
    }
    assert_eq!(px, FONT_MAX);
    for _ in 0..40 {
        px = font_step(px, -1);
    }
    assert_eq!(px, FONT_MIN);
}

#[test]
fn every_possible_input_byte_parses_without_panic() {
    // Poor man's fuzz: short strings over a nasty alphabet.
    let alphabet = b"=\n#- 0123456789abcxyz\r\xFF";
    let mut buf = [0u8; 5];
    fn walk(depth: usize, buf: &mut [u8; 5], alphabet: &[u8]) {
        if depth == buf.len() {
            let _ = Settings::parse(buf);
            return;
        }
        for &a in alphabet {
            buf[depth] = a;
            walk(depth + 1, buf, alphabet);
        }
    }
    walk(0, &mut buf, alphabet);
}

#[test]
fn stored_wallpaper_paths_become_absolute_volume_paths() {
    assert_eq!(absolute_path(b"papel.png"), b"/papel.png");
    assert_eq!(absolute_path(b"fotos/praia.png"), b"/fotos/praia.png");
    assert_eq!(absolute_path(b"/Imagens/a.png"), b"/Imagens/a.png");
    // Whatever the settings file holds, the result is a single leading slash
    // followed by the stored text: nothing is resolved or removed here (the
    // volume rejects `.`/`..` components itself).
    let mut s = Settings::new();
    assert!(s.set_image_path(b"a/b.png"));
    assert_eq!(absolute_path(s.image_path()), b"/a/b.png");
}

#[test]
fn notification_time_and_dock_zoom_parse_within_range() {
    let s = Settings::parse(b"toast_secs=9\ndock_zoom=40\n");
    assert_eq!((s.toast_secs, s.dock_zoom), (9, 40));
    // Out of range, negative or garbage keeps the default.
    for bad in [
        &b"toast_secs=1\n"[..],
        b"toast_secs=16\n",
        b"toast_secs=-3\n",
        b"toast_secs=x\n",
    ] {
        assert_eq!(
            Settings::parse(bad).toast_secs,
            TOAST_SECS_DEFAULT,
            "{bad:?}"
        );
    }
    for bad in [&b"dock_zoom=101\n"[..], b"dock_zoom=-1\n", b"dock_zoom=\n"] {
        assert_eq!(Settings::parse(bad).dock_zoom, DOCK_ZOOM_MAX, "{bad:?}");
    }
    let edge = Settings::parse(b"toast_secs=2\ndock_zoom=0\n");
    assert_eq!((edge.toast_secs, edge.dock_zoom), (2, 0));
    let edge = Settings::parse(b"toast_secs=15\ndock_zoom=100\n");
    assert_eq!((edge.toast_secs, edge.dock_zoom), (15, 100));
}

#[test]
fn new_fields_roundtrip() {
    let mut s = Settings {
        toast_secs: 11,
        dock_zoom: 35,
        ..Settings::default()
    };
    s.set_city(city_for_offset(540));
    let t = s.to_text();
    assert_eq!(Settings::parse(&t), s);
    assert_eq!(Settings::parse(&t).tz_minutes, 540);
    assert!(t.len() < 300, "{}", t.len());
}

#[test]
fn the_city_must_agree_with_the_offset() {
    // A stored city that does not match the offset is dropped for the offset's own.
    let s = Settings::parse(b"tz=-180\ntz_city=43\n");
    assert_eq!(s.tz_minutes, -180);
    assert_eq!(city_name_in(Lang::Pt, s.tz_city), "Brasília");
    // An offset that no city has leaves the picker without a selection.
    let s = Settings::parse(b"tz=-30\n");
    assert_eq!(s.tz_city, CITY_NONE);
    // Out-of-range indices are ignored.
    let s = Settings::parse(b"tz=60\ntz_city=200\n");
    assert_eq!(TIMEZONES[s.tz_city as usize].1, 60);
    // The chosen city sticks while the offset agrees.
    let s = Settings::parse(b"tz=0\ntz_city=22\n");
    assert_eq!(city_name_in(Lang::En, s.tz_city), "Reykjavik");
    // set_city refuses an index outside the list.
    let mut s = Settings::default();
    s.set_city(250);
    assert_eq!(s.tz_minutes, -180);
}

#[test]
fn the_city_table_is_sorted_valid_and_findable() {
    assert!(
        TIMEZONES
            .windows(2)
            .all(|w| w[0].1 <= w[1].1 || w[1].0 == tk!("settings.tz.chatham"))
    );
    assert_eq!(city_name_in(Lang::Pt, DEFAULT_CITY), "Brasília");
    for (i, &(_, m)) in TIMEZONES.iter().enumerate() {
        for l in Lang::ALL {
            assert!(!city_name_in(l, i as u8).is_empty(), "{i}");
        }
        assert!((TZ_MIN..=TZ_MAX).contains(&(m as i32)), "{i}");
    }
    assert_eq!(utc_label(-180).as_bytes(), b"UTC-03:00");
    assert_eq!(utc_label(330).as_bytes(), b"UTC+05:30");
    assert_eq!(utc_label(0).as_bytes(), b"UTC+00:00");
    assert_eq!(search_timezones("").len(), TIMEZONES.len());
    let r = search_timezones("sao");
    assert_eq!(r.len(), 1);
    assert_eq!(city_name_in(Lang::Pt, r[0]), "São Paulo");
    // Both names of a city find it, whatever the language in effect.
    assert_eq!(search_timezones("TOKYO"), search_timezones("toquio"));
    assert_eq!(search_timezones("toquio").len(), 1);
    assert_eq!(search_timezones("lisbon"), search_timezones("Lisboa"));
    assert_eq!(search_timezones("lisbon").len(), 1);
    // The offset text finds cities too.
    assert!(search_timezones("-03:00").len() >= 3);
    assert!(search_timezones("zzzz").is_empty());
    assert_eq!(city_for_offset(-180), DEFAULT_CITY);
    assert_eq!(city_for_offset(840), 51);
    assert_eq!(city_for_offset(1), CITY_NONE);
}

#[test]
fn auto_lock_round_trips_and_ignores_nonsense() {
    assert_eq!(Settings::new().auto_lock_min, 0);
    assert!(
        !String::from_utf8(Settings::new().to_text())
            .unwrap()
            .contains("auto_lock")
    );
    let mut s = Settings::new();
    s.auto_lock_min = 15;
    let back = Settings::parse(&s.to_text());
    assert_eq!(back.auto_lock_min, 15);
    for bad in [&b"auto_lock=999\n"[..], b"auto_lock=-1\n", b"auto_lock=x\n"] {
        assert_eq!(Settings::parse(bad).auto_lock_min, 0);
    }
    assert!(AUTO_LOCK_CHOICES.contains(&0) && AUTO_LOCK_CHOICES.contains(&15));
}

#[test]
fn ad_block_is_on_by_default_and_round_trips() {
    assert!(Settings::new().ad_block);
    assert!(Settings::parse(b"").ad_block);
    let mut s = Settings::new();
    s.ad_block = false;
    let text = s.to_text();
    assert!(
        String::from_utf8(text.clone())
            .unwrap()
            .contains("adblock=0")
    );
    assert!(!Settings::parse(&text).ad_block);
    assert!(Settings::parse(b"adblock=0\nadblock=on\n").ad_block);
    assert!(!Settings::parse(b"adblock=off\n").ad_block);
    // Nonsense keeps the default.
    assert!(Settings::parse(b"adblock=maybe\n").ad_block);
    assert!(Settings::parse(b"adblock=\n").ad_block);
    assert_eq!(Settings::parse(&Settings::new().to_text()), Settings::new());
}
