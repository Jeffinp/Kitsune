use crate::browsing::media::player::{Event, Player};
use crate::browsing::media::proto::{self, Field, Header, Out, parse_out};
use crate::browsing::media::wire::*;
use alloc::string::String;
use alloc::vec;

#[test]
fn controls_from_the_page() {
    let c = |s: &str| parse_control(s.as_bytes()).map(|c| (c.id, c.control));
    assert_eq!(c("open 7"), Some((7, Control::Open)));
    assert_eq!(
        c("seek 7 1500000000"),
        Some((7, Control::Seek(1_500_000_000)))
    );
    assert_eq!(c("seek 7 -5"), Some((7, Control::Seek(0))));
    assert_eq!(c("vol 7 250 1"), Some((7, Control::Volume(100, true))));
    assert_eq!(c("dur 7 -1"), Some((7, Control::Duration(None))));
    assert_eq!(
        c("rm 7 1 0 100"),
        Some((
            7,
            Control::Remove {
                src: 1,
                a: 0,
                b: 100
            }
        ))
    );
    assert_eq!(
        c("reset 7 0 4096"),
        Some((
            7,
            Control::Reset {
                src: 0,
                offset: 4096
            }
        ))
    );
    assert_eq!(
        c("reset 7 1 -1"),
        Some((
            7,
            Control::Reset {
                src: 1,
                offset: u64::MAX
            }
        ))
    );
    assert_eq!(
        c("tso 7 2 -1000"),
        Some((7, Control::TimestampOffset { src: 2, ns: -1000 }))
    );
    // Malformed: unknown op, missing numbers, a source out of range.
    for bad in ["fly 7", "seek 7", "open", "rm 7 9 0 1", "seek x 1", ""] {
        assert_eq!(c(bad), None, "{bad}");
    }
    assert_eq!(
        c("src 7 https://h/v.webm?a=1&b=2"),
        Some((7, Control::Src(String::from("https://h/v.webm?a=1&b=2"))))
    );
    assert_eq!(c("src 7 "), None);
    assert_eq!(c("src x http://h/"), None);
    let long_url = String::from("src 1 http://h/") + &"a".repeat(3000);
    assert!(c(&long_url).is_some(), "long addresses are fine");
    let long = String::from("play 1 ") + &"9".repeat(300);
    assert_eq!(c(&long), None);
}

#[test]
fn data_from_the_page() {
    assert_eq!(
        parse_data(b"12 1 99\n\x1a\x45"),
        Some((12, 1, 99, &b"\x1a\x45"[..]))
    );
    assert_eq!(parse_data(b"12 1 99\n"), Some((12, 1, 99, &b""[..])));
    assert_eq!(parse_data(b"12 9 99\nxx"), None);
    assert_eq!(parse_data(b"12 1\nxx"), None);
    assert_eq!(parse_data(b"no newline"), None);
}

#[test]
fn state_and_buffered_messages() {
    let mut p: Player<u32> = Player::new(Default::default(), false);
    p.header(Header {
        duration_ns: 3_000_000_000,
        width: 640,
        height: 360,
        video: true,
        audio: true,
        channels: 2,
        src: 0,
    });
    let ev = p.take_events();
    let m = String::from_utf8(state_message(5, p.state(), 1234, &ev, (7, 2))).unwrap();
    assert!(
        m.starts_with("{\"t\":\"media\",\"id\":5,\"ev\":[\"durationchange\",\"loadedmetadata\"]"),
        "{m}"
    );
    assert!(m.contains("\"ct\":1234") && m.contains("\"w\":640") && m.contains("\"d\":3000000000"));
    assert!(m.contains("\"p\":1") && m.contains("\"rs\":1"));
    assert!(m.ends_with(",\"fs\":7,\"fd\":2}"), "{m}");
    let b = String::from_utf8(buffered_message(5, 1, 3, 0, &[(0, 10), (20, 30)])).unwrap();
    assert_eq!(
        b,
        "{\"t\":\"media\",\"id\":5,\"buf\":[1,3,0,[[0,10],[20,30]]]}"
    );
    assert_eq!(
        String::from_utf8(cue_message(5, 0, 7, None)).unwrap(),
        "{\"t\":\"media\",\"id\":5,\"cue\":[0,7,-1]}"
    );
    let e = String::from_utf8(error_message(5, 4, "no \"free\" player")).unwrap();
    assert_eq!(
        e,
        "{\"t\":\"media\",\"id\":5,\"fail\":[4,\"no \\\"free\\\" player\"]}"
    );
    assert_eq!(Event::CanPlayThrough.name(), "canplaythrough");
}

#[test]
fn decoder_messages_round_trip_and_hostile_ones_are_refused() {
    let h = Header {
        duration_ns: 5,
        width: 2,
        height: 1,
        video: true,
        audio: false,
        channels: 0,
        src: 3,
    };
    assert_eq!(parse_out(&proto::header(&h)), Some(Out::Header(h)));
    let mut f = vec![];
    proto::frame_head(&mut f, -7, 2, 1, true, 3);
    f.extend_from_slice(&[1, 2, 3, 255, 4, 5, 6, 255]);
    assert_eq!(
        parse_out(&f),
        Some(Out::Frame {
            pts: -7,
            w: 2,
            h: 1,
            bgr: true,
            skipped: 3,
            rgba: &[1, 2, 3, 255, 4, 5, 6, 255]
        })
    );
    // Pixels missing, or a size beyond the limits.
    assert_eq!(parse_out(&f[..f.len() - 1]), None);
    let mut big = vec![];
    proto::frame_head(&mut big, 0, 5000, 1, false, 0);
    assert_eq!(parse_out(&big), None);
    let mut a = vec![];
    proto::audio_head(&mut a, 9, 1);
    a.extend_from_slice(&[0, 1, 2, 3]);
    assert!(matches!(
        parse_out(&a),
        Some(Out::Audio {
            pts: 9,
            frames: 1,
            ..
        })
    ));
    a.push(0);
    assert_eq!(parse_out(&a), None);
    let b = proto::buffered(1, 2, 0, &[(0, 5), (9, 12)]);
    assert_eq!(
        parse_out(&b),
        Some(Out::Buffered {
            src: 1,
            seq: 2,
            status: 0,
            ranges: vec![(0, 5), (9, 12)]
        })
    );
    assert_eq!(parse_out(&b[..b.len() - 3]), None);
    let m = proto::msg(proto::OUT_MARK, &[Field::U32(4)]);
    assert_eq!(parse_out(&m), Some(Out::Mark { epoch: 4 }));
    let e = proto::error(2, "bad");
    assert_eq!(
        parse_out(&e),
        Some(Out::Error {
            code: 2,
            text: "bad"
        })
    );
    assert_eq!(parse_out(&[0x99]), None);
    assert_eq!(parse_out(&[]), None);
    let f = proto::msg(
        proto::OUT_FETCH,
        &[
            Field::U32(3),
            Field::U8(proto::FETCH_END),
            Field::U32(206),
            Field::U64(u64::MAX),
            Field::U64(1000),
        ],
    );
    assert_eq!(
        parse_out(&f),
        Some(Out::Fetch {
            id: 3,
            ev: proto::FETCH_END,
            status: 206,
            total: None,
            bytes: 1000
        })
    );
    assert_eq!(proto::fit((0, 0), 1920, 1080), (1280, 720));
    assert_eq!(proto::fit((0, 0), 640, 360), (640, 360));
    assert_eq!(proto::fit((300, 150), 640, 360), (300, 150));
}

#[test]
fn the_fetchers_answers_for_the_decoder() {
    use crate::browsing::web::script::{
        FetchHead, ResponseType, end_message, error_message, head_message,
    };
    let headers = vec![
        (String::from("Content-Type"), String::from("video/webm")),
        (
            String::from("Content-Range"),
            String::from("bytes 0-1048575/1052940"),
        ),
    ];
    let head = head_message(&FetchHead {
        id: 9,
        status: 206,
        status_text: "Partial Content",
        url: "https://h/v.webm",
        redirected: false,
        kind: ResponseType::Basic,
        headers: &headers,
        cookie: "",
    });
    assert_eq!(
        proto::fetch_event(&head),
        Some((9, proto::FETCH_HEAD, 206, Some(1_052_940)))
    );
    let whole = vec![(String::from("content-length"), String::from("3000"))];
    let head = head_message(&FetchHead {
        id: 10,
        status: 200,
        status_text: "OK",
        url: "u",
        redirected: false,
        kind: ResponseType::Basic,
        headers: &whole,
        cookie: "",
    });
    assert_eq!(
        proto::fetch_event(&head),
        Some((10, proto::FETCH_HEAD, 200, Some(3000)))
    );
    assert_eq!(
        proto::fetch_event(&end_message(9)),
        Some((9, proto::FETCH_END, 0, None))
    );
    assert_eq!(
        proto::fetch_event(&error_message(9, "boom")).map(|e| e.1),
        Some(proto::FETCH_FAIL)
    );
    assert_eq!(proto::fetch_event(b"{\"t\":\"media\"}"), None);
    assert_eq!(proto::fetch_event(b"\xff"), None);
}
