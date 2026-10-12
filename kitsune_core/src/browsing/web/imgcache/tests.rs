use super::*;

fn png(w: usize, h: usize) -> Vec<u8> {
    let img = Image::new(w, h, 0xFF30_60C0).unwrap();
    crate::format::png::encode(&img).unwrap()
}

fn loaded(w: usize, h: usize) -> Loaded {
    Loaded {
        orig_w: w,
        orig_h: h,
        img: Image::new(w, h, 0xFF00_0000).unwrap(),
    }
}

#[test]
fn decode_small_png_keeps_size() {
    let l = decode_for_page(&png(40, 30), 300).unwrap();
    assert_eq!((l.orig_w, l.orig_h), (40, 30));
    assert_eq!((l.img.width(), l.img.height()), (40, 30));
    assert!(l.img.is_opaque());
}

#[test]
fn decode_scales_down_to_the_column() {
    let l = decode_for_page(&png(400, 200), 100).unwrap();
    assert_eq!((l.orig_w, l.orig_h), (400, 200));
    assert_eq!((l.img.width(), l.img.height()), (100, 50));
}

#[test]
fn small_images_are_not_enlarged() {
    let l = decode_for_page(&png(20, 20), 800).unwrap();
    assert_eq!(l.img.width(), 20);
}

#[test]
fn bmp_and_ppm_decode() {
    let img = Image::new(8, 6, 0xFFFF_0000).unwrap();
    let bmp = image::encode(&img, Format::Bmp).unwrap();
    assert_eq!(decode_for_page(&bmp, 100).unwrap().orig_w, 8);
    let ppm = image::encode(&img, Format::Ppm).unwrap();
    assert_eq!(decode_for_page(&ppm, 100).unwrap().orig_h, 6);
}

#[test]
fn webp_svg_and_empty_are_unsupported() {
    for sig in [&b"RIFF\0\0\0\0WEBPVP8 "[..], b"<svg xmlns=", b""] {
        assert_eq!(decode_for_page(sig, 100).err(), Some(ImgFail::Unsupported));
    }
}

#[test]
fn svg_pictures_decode_at_their_size_or_the_column() {
    let svg = b"<svg xmlns='http://www.w3.org/2000/svg' width='200' height='100'><rect width='200' height='100' fill='#336699'/></svg>";
    assert_eq!(peek_dims(svg), Some((200, 100)));
    let l = decode_for_page(svg, 1000).unwrap();
    assert_eq!((l.orig_w, l.orig_h), (200, 100));
    assert_eq!((l.img.width(), l.img.height()), (200, 100));
    let l = decode_for_page(svg, 50).unwrap();
    assert_eq!((l.orig_w, l.img.width(), l.img.height()), (200, 50, 25));
    // Opaque after flattening over the page colour.
    assert!(l.img.is_opaque());
    assert_eq!(
        decode_for_page(b"<svg width='99999' height='9'/>", 100).err(),
        Some(ImgFail::TooBig)
    );
}

#[test]
fn truncated_png_fails_cleanly() {
    let p = png(50, 50);
    assert_eq!(
        decode_for_page(&p[..p.len() / 2], 100).err(),
        Some(ImgFail::Failed)
    );
    assert_eq!(decode_for_page(&p[..9], 100).err(), Some(ImgFail::Failed));
}

#[test]
fn body_over_the_byte_limit_is_too_big() {
    let mut p = png(10, 10);
    p.resize(MAX_IMAGE_BYTES + 1, 0);
    assert_eq!(decode_for_page(&p, 100).err(), Some(ImgFail::TooBig));
}

#[test]
fn pixel_limit_is_checked_on_the_header() {
    // A 4000x3000 PNG header with no pixel data: refused as too big before
    // anything is allocated or decoded.
    let big = png(2100, 1000); // 2.1 Mpx, valid and small once compressed
    assert!(big.len() < MAX_IMAGE_BYTES);
    assert_eq!(decode_for_page(&big, 100).err(), Some(ImgFail::TooBig));
    let ok = png(1400, 1000); // 1.4 Mpx
    assert!(decode_for_page(&ok, 700).is_ok());
}

#[test]
fn peek_dims_of_each_format() {
    assert_eq!(peek_dims(&png(7, 9)), Some((7, 9)));
    let img = Image::new(5, 4, 0xFF00_FF00).unwrap();
    assert_eq!(
        peek_dims(&image::encode(&img, Format::Bmp).unwrap()),
        Some((5, 4))
    );
    assert_eq!(
        peek_dims(&image::encode(&img, Format::Ppm).unwrap()),
        Some((5, 4))
    );
    assert_eq!(peek_dims(b"P6 # c\n12 34\n255\n"), Some((12, 34)));
    assert_eq!(peek_dims(b"junk"), None);
    assert_eq!(peek_dims(b"BM"), None);
    assert_eq!(peek_dims(b"P6 99999999999999999999999 1"), None);
}

#[test]
fn transparent_png_is_flattened() {
    let mut img = Image::new(4, 4, 0x0000_0000).unwrap();
    img.set(0, 0, 0xFF10_2030);
    let bytes = image::encode(&img, Format::Png).unwrap();
    let l = decode_for_page(&bytes, 100).unwrap();
    assert!(l.img.is_opaque());
    assert_eq!(l.img.get(1, 1), Some(PAGE_BG));
    assert_eq!(l.img.get(0, 0), Some(0xFF10_2030));
}

#[test]
fn data_uri_decodes_a_png() {
    let uri = alloc::format!("data:image/png;base64,{}", base64::encode(&png(10, 10)));
    let l = decode_data_uri(&uri, 100).unwrap();
    assert_eq!(l.orig_w, 10);
}

#[test]
fn data_uri_errors() {
    assert_eq!(
        decode_data_uri("data:image/jpeg;base64,/9j/4AAQ", 100).err(),
        Some(ImgFail::Failed),
        "a JPEG header with nothing after it is damaged, not unsupported"
    );
    assert_eq!(
        decode_data_uri("data:image/png;base64,!!!!", 100).err(),
        Some(ImgFail::Failed)
    );
    assert_eq!(
        decode_data_uri("data:text/plain,hello", 100).err(),
        Some(ImgFail::Unsupported)
    );
    let huge = alloc::format!("data:image/png;base64,{}", "A".repeat(MAX_DATA_URI_LEN));
    assert_eq!(decode_data_uri(&huge, 100).err(), Some(ImgFail::TooBig));
    let over = alloc::format!(
        "data:image/png;base64,{}",
        base64::encode(&alloc::vec![0u8; MAX_DATA_IMAGE_BYTES + 3])
    );
    assert_eq!(decode_data_uri(&over, 100).err(), Some(ImgFail::TooBig));
}

#[test]
fn keys_resolve_against_the_page() {
    let base = b"http://h.test/a/b.html";
    assert_eq!(
        image_key(base, "p.png").as_deref(),
        Some("http://h.test/a/p.png")
    );
    assert_eq!(
        image_key(base, "/x/y.png").as_deref(),
        Some("http://h.test/x/y.png")
    );
    assert_eq!(
        image_key(base, "//cdn.test/z.png").as_deref(),
        Some("http://cdn.test/z.png")
    );
    assert_eq!(
        image_key(base, "http://o.test/i.png").as_deref(),
        Some("http://o.test/i.png")
    );
}

#[test]
fn mixed_content_and_junk_have_no_key() {
    let https = b"https://h.test/";
    assert_eq!(image_key(https, "http://h.test/i.png"), None);
    assert_eq!(image_key(https, ""), None);
    assert_eq!(image_key(https, "   "), None);
    assert_eq!(image_key(https, "javascript:alert(1)"), None);
    assert_eq!(image_key(https, "ftp://h/i.png"), None);
    assert_eq!(image_key(b"", "a.png"), None);
}

#[test]
fn data_keys_are_short_and_stable() {
    let uri = alloc::format!("data:image/png;base64,{}", "QUJD".repeat(5000));
    let k = image_key(b"http://h/", &uri).unwrap();
    assert!(k.starts_with("data:#") && k.len() < 40);
    assert_eq!(image_key(b"http://other/", &uri).unwrap(), k);
    let other = alloc::format!("data:image/png;base64,{}", "QUJE".repeat(5000));
    assert_ne!(image_key(b"http://h/", &other).unwrap(), k);
    assert_eq!(image_key(b"http://h/", "data:nocomma"), None);
}

#[test]
fn want_registers_pending_and_caps_the_page() {
    let mut c = ImageCache::new();
    c.begin_page();
    for i in 0..MAX_PAGE_IMAGES {
        assert!(c.want(&alloc::format!("http://h/{i}.png"), None));
    }
    assert!(!c.want("http://h/extra.png", None));
    assert_eq!(c.state("http://h/extra.png"), ImgState::TooMany);
    assert_eq!(c.state("http://h/0.png"), ImgState::Pending);
    // Asking again for one already there is fine.
    assert!(c.want("http://h/3.png", None));
}

#[test]
fn pending_images_are_handed_out_one_at_a_time_in_order() {
    let mut c = ImageCache::new();
    c.begin_page();
    c.want("a", None);
    c.want("b", None);
    assert_eq!(c.next_pending().unwrap().0, "a");
    assert!(c.next_pending().is_none(), "one in flight at a time");
    c.finish("a", Ok(loaded(4, 4)));
    assert_eq!(c.next_pending().unwrap().0, "b");
    c.finish("b", Err(ImgFail::Failed));
    assert!(c.next_pending().is_none());
    assert!(!c.busy());
}

#[test]
fn states_follow_the_outcome() {
    let mut c = ImageCache::new();
    c.begin_page();
    for k in ["ok", "jpg", "big", "bad"] {
        c.want(k, None);
    }
    c.finish("ok", Ok(loaded(30, 20)));
    c.finish("jpg", Err(ImgFail::Unsupported));
    c.finish("big", Err(ImgFail::TooBig));
    c.finish("bad", Err(ImgFail::Failed));
    assert_eq!(c.state("ok"), ImgState::Ready { w: 30, h: 20 });
    assert_eq!(c.state("jpg"), ImgState::Unsupported);
    assert_eq!(c.state("big"), ImgState::TooBig);
    assert_eq!(c.state("bad"), ImgState::Failed);
    assert!(c.image("ok").is_some() && c.image("jpg").is_none());
}

#[test]
fn messages_exist_only_for_failures() {
    assert_eq!(ImgState::Pending.message(), None);
    assert_eq!(ImgState::Ready { w: 1, h: 1 }.message(), None);
    assert_eq!(
        ImgState::Unsupported.message_key(),
        Some("web.img.unsupported")
    );
    for s in [
        ImgState::Unsupported,
        ImgState::Failed,
        ImgState::TooBig,
        ImgState::TooMany,
    ] {
        for l in crate::i18n::Lang::ALL {
            let t = crate::i18n::tr_in(l, s.message_key().unwrap());
            assert!(!t.is_empty() && !t.starts_with("web."), "{l:?}");
        }
    }
    assert_eq!(
        crate::i18n::tr_in(crate::i18n::Lang::En, "web.img.unsupported"),
        "unsupported format"
    );
    assert_eq!(
        crate::i18n::tr_in(crate::i18n::Lang::Pt, "web.img.unsupported"),
        "formato não suportado"
    );
}

#[test]
fn byte_budget_evicts_the_least_recently_used() {
    let mut c = ImageCache::new();
    // Each picture is 1000x600 = 2.4 MB; four do not fit in 8 MiB.
    for i in 0..4 {
        c.begin_page();
        let k = alloc::format!("p{i}");
        c.want(&k, None);
        c.finish(&k, Ok(loaded(1000, 600)));
    }
    assert!(c.bytes() <= CACHE_BYTES);
    assert!(c.image("p3").is_some(), "the newest survives");
    assert!(c.image("p0").is_none(), "the oldest was evicted");
}

#[test]
fn evicting_a_page_image_leaves_a_stub_not_a_refetch() {
    let mut c = ImageCache::new();
    c.begin_page();
    for i in 0..5 {
        let k = alloc::format!("q{i}");
        c.want(&k, None);
        c.finish(&k, Ok(loaded(1000, 700)));
    }
    assert!(c.bytes() <= CACHE_BYTES);
    // Nothing of this page went back to Pending (that would download again).
    assert!(!c.busy());
    assert!((0..5).any(|i| c.state(&alloc::format!("q{i}")) == ImgState::TooBig));
}

#[test]
fn entry_count_is_bounded() {
    let mut c = ImageCache::new();
    for i in 0..100 {
        c.begin_page();
        c.want(&alloc::format!("e{i}"), None);
    }
    assert!(c.len() <= MAX_ENTRIES);
}

#[test]
fn fit_to_rescales_once() {
    let mut c = ImageCache::new();
    c.begin_page();
    c.want("a", None);
    c.finish("a", Ok(loaded(100, 50)));
    assert!(c.fit_to("a", 50, 25));
    assert!(!c.fit_to("a", 50, 25));
    let i = c.image("a").unwrap();
    assert_eq!((i.width(), i.height()), (50, 25));
    assert_eq!(c.state("a"), ImgState::Ready { w: 100, h: 50 });
    assert!(!c.fit_to("a", 0, 5));
    assert!(!c.fit_to("a", 5000, 5));
    assert!(!c.fit_to("zzz", 5, 5));
}

#[test]
fn data_uri_is_kept_until_decoded() {
    let mut c = ImageCache::new();
    c.begin_page();
    c.want("data:#1-2", Some("data:image/png;base64,AA=="));
    let (k, d) = c.next_pending().unwrap();
    assert_eq!(d.as_deref(), Some("data:image/png;base64,AA=="));
    c.finish(&k, Err(ImgFail::Failed));
    c.begin_page();
    c.want(&k, None);
    // After finishing, the URI text is gone.
    assert!(c.next_pending().is_none());
}

#[test]
fn loaded_images_survive_into_the_next_page() {
    let mut c = ImageCache::new();
    c.begin_page();
    c.want("u", None);
    c.finish("u", Ok(loaded(10, 10)));
    c.begin_page();
    assert!(c.want("u", None));
    assert_eq!(c.state("u"), ImgState::Ready { w: 10, h: 10 });
    assert!(c.next_pending().is_none(), "no second download");
}

#[test]
fn requeue_puts_loading_back() {
    let mut c = ImageCache::new();
    c.begin_page();
    c.want("u", None);
    assert!(c.next_pending().is_some());
    assert!(c.next_pending().is_none());
    c.requeue_loading();
    assert!(c.next_pending().is_some());
}

#[test]
fn clear_empties_everything() {
    let mut c = ImageCache::new();
    c.begin_page();
    c.want("u", None);
    c.finish("u", Ok(loaded(5, 5)));
    c.clear();
    assert!(c.is_empty() && c.bytes() == 0);
}

#[test]
fn page_images_resolve_then_look_up() {
    let mut c = ImageCache::new();
    c.begin_page();
    let k = image_key(b"http://h/a/", "i.png").unwrap();
    c.want(&k, None);
    c.finish(&k, Ok(loaded(12, 8)));
    let pi = PageImages {
        cache: &c,
        base: b"http://h/a/index.html",
    };
    assert_eq!(pi.lookup("i.png"), ImgState::Ready { w: 12, h: 8 });
    // Not registered yet, and the page still has room: it will be fetched.
    assert_eq!(pi.lookup("other.png"), ImgState::Pending);
    assert_eq!(pi.lookup(""), ImgState::Failed);
    assert_eq!(NoImages.lookup("x"), ImgState::Pending);
}

#[test]
fn finish_of_an_unknown_key_is_ignored() {
    let mut c = ImageCache::new();
    c.finish("nope", Ok(loaded(5, 5)));
    assert!(c.is_empty());
}

#[test]
fn gif_and_jpeg_pages_decode() {
    let jpg = include_bytes!("../../../format/jpeg/testdata/s420.jpg");
    let l = decode_for_page(jpg, 100).unwrap();
    assert_eq!((l.orig_w, l.orig_h), (37, 21));
    assert!(l.img.is_opaque());
    assert_eq!(peek_dims(jpg), Some((37, 21)));
    // The 1x1 transparent GIF: transparent pixels are flattened onto the page background.
    let gif = [
        0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0xff, 0xff,
        0xff, 0x00, 0x00, 0x00, 0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00,
        0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44, 0x01, 0x00, 0x3b,
    ];
    let l = decode_for_page(&gif, 100).unwrap();
    assert_eq!((l.orig_w, l.orig_h), (1, 1));
    assert!(l.img.is_opaque());
}

#[test]
fn cmyk_jpeg_is_unsupported_and_progressive_is_an_image() {
    let cmyk = &include_bytes!("../../../format/jpeg/testdata/cmyk.jpg")[..];
    assert_eq!(decode_for_page(cmyk, 100).err(), Some(ImgFail::Unsupported));
    // A progressive file is just an image; a huge one is stopped by the page's pixel limit
    // (4000 x 4000 patched into a real header) before its coefficients are allocated.
    let ok = include_bytes!("../../../format/jpeg/testdata/prog_420.jpg");
    assert!(decode_for_page(ok, 100).is_ok());
    let mut huge = ok.to_vec();
    let sof = huge.windows(2).position(|w| w == [0xFF, 0xC2]).unwrap();
    huge[sof + 5..sof + 9].copy_from_slice(&[0x0F, 0xA0, 0x0F, 0xA0]);
    assert_eq!(decode_for_page(&huge, 100).err(), Some(ImgFail::TooBig));
}

#[test]
fn damaged_gif_and_jpeg_fail_cleanly() {
    let jpg = include_bytes!("../../../format/jpeg/testdata/s420.jpg");
    assert_eq!(
        decode_for_page(&jpg[..30], 100).err(),
        Some(ImgFail::Failed)
    );
    assert_eq!(
        decode_for_page(b"GIF89a\x01", 100).err(),
        Some(ImgFail::Failed)
    );
}
