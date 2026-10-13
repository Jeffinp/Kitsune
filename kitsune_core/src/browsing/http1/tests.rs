use super::*;
use alloc::vec;

/// Feed `parts` one after the other; returns (head, body, bytes used, done, reusable).
fn run(head_req: bool, parts: &[&[u8]]) -> (Vec<u8>, Vec<u8>, usize, bool, bool) {
    let mut r = ResponseReader::new(head_req);
    let (mut head, mut body, mut used) = (Vec::new(), Vec::new(), 0);
    for p in parts {
        used += r
            .feed(p, &mut |part| match part {
                Part::Head(h) => head = h.to_vec(),
                Part::Body(b) => body.extend_from_slice(b),
            })
            .unwrap();
    }
    (head, body, used, r.is_done(), r.reusable())
}

/// Feed `data` one byte at a time.
fn bytewise(data: &[u8]) -> (Vec<u8>, bool, bool) {
    let parts: Vec<&[u8]> = data.chunks(1).collect();
    let (_, body, _, done, reuse) = run(false, &parts);
    (body, done, reuse)
}

#[test]
fn content_length_ends_the_response() {
    let msg = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhelloEXTRA";
    let (head, body, used, done, reuse) = run(false, &[msg]);
    assert!(head.starts_with(b"HTTP/1.1 200 OK"));
    assert!(head.ends_with(b"\r\n\r\n"));
    assert_eq!(body, b"hello");
    assert_eq!(used, msg.len() - 5);
    assert!(done && reuse);
}

#[test]
fn split_anywhere_gives_the_same_body() {
    let msg = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5;ext=1\r\nhello\r\n0A\r\n0123456789\r\n0\r\nX-T: 1\r\n\r\n";
    let (body, done, reuse) = bytewise(msg);
    assert_eq!(body, b"hello0123456789");
    assert!(done && reuse);
    for cut in 0..msg.len() {
        let (_, body, used, done, _) = run(false, &[&msg[..cut], &msg[cut..]]);
        assert_eq!(body, b"hello0123456789", "cut at {cut}");
        assert_eq!(used, msg.len());
        assert!(done);
    }
}

#[test]
fn chunked_with_bare_newlines_and_uppercase_hex() {
    let msg = b"HTTP/1.1 200 OK\nTransfer-Encoding: gzip, Chunked\n\nA\n0123456789\n0\n\n";
    let (body, done, _) = bytewise(msg);
    assert_eq!(body, b"0123456789");
    assert!(done);
}

#[test]
fn bad_chunk_size_is_an_error() {
    let mut r = ResponseReader::new(false);
    let e = r.feed(
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",
        &mut |_| {},
    );
    assert_eq!(e, Err(HttpError::BadChunk));
    let mut r = ResponseReader::new(false);
    let e = r.feed(
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nFFFFFFFFFFFFFFFFFF\r\n",
        &mut |_| {},
    );
    assert_eq!(e, Err(HttpError::BadChunk));
    // Data not followed by CRLF.
    let mut r = ResponseReader::new(false);
    let e = r.feed(
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nab",
        &mut |_| {},
    );
    assert_eq!(e, Err(HttpError::BadChunk));
}

#[test]
fn no_length_reads_until_close_and_is_not_reused() {
    let mut r = ResponseReader::new(false);
    let mut body = Vec::new();
    r.feed(b"HTTP/1.1 200 OK\r\n\r\nabc", &mut |p| {
        if let Part::Body(b) = p {
            body.extend_from_slice(b)
        }
    })
    .unwrap();
    assert!(!r.is_done());
    assert_eq!(r.eof(), Ok(()));
    assert!(r.is_done() && !r.reusable());
    assert_eq!(body, b"abc");
}

#[test]
fn early_close_is_incomplete() {
    let mut r = ResponseReader::new(false);
    r.feed(
        b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc",
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(r.eof(), Err(HttpError::Incomplete));
    let mut r = ResponseReader::new(false);
    r.feed(b"HTTP/1.1 200", &mut |_| {}).unwrap();
    assert_eq!(r.eof(), Err(HttpError::Incomplete));
}

#[test]
fn head_and_no_content_have_no_body() {
    let (_, body, _, done, reuse) = run(true, &[b"HTTP/1.1 200 OK\r\nContent-Length: 99\r\n\r\n"]);
    assert!(body.is_empty() && done && reuse);
    let (_, _, _, done, _) = run(false, &[b"HTTP/1.1 204 No Content\r\n\r\n"]);
    assert!(done);
    let (_, _, _, done, reuse) = run(
        false,
        &[b"HTTP/1.1 304 Not Modified\r\nTransfer-Encoding: chunked\r\n\r\n"],
    );
    assert!(done && reuse);
}

#[test]
fn interim_answers_are_skipped() {
    let msg = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 103 Early Hints\r\nLink: </a.css>\r\n\r\nHTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\nok";
    let (head, body, _, done, _) = run(false, &[msg]);
    assert!(head.starts_with(b"HTTP/1.1 201"));
    assert_eq!(body, b"ok");
    assert!(done);
}

#[test]
fn connection_close_and_http10() {
    let (_, _, _, done, reuse) = run(
        false,
        &[b"HTTP/1.1 200 OK\r\nConnection: Close\r\nContent-Length: 1\r\n\r\nx"],
    );
    assert!(done && !reuse);
    let (_, _, _, done, reuse) = run(false, &[b"HTTP/1.0 200 OK\r\nContent-Length: 1\r\n\r\nx"]);
    assert!(done && !reuse);
    let (_, _, _, _, reuse) = run(
        false,
        &[b"HTTP/1.0 200 OK\r\nConnection: keep-alive\r\nContent-Length: 1\r\n\r\nx"],
    );
    assert!(reuse);
}

#[test]
fn conflicting_lengths_are_refused() {
    let mut r = ResponseReader::new(false);
    let e = r.feed(
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nContent-Length: 4\r\n\r\n",
        &mut |_| {},
    );
    assert_eq!(e, Err(HttpError::BadLength));
    let (_, body, _, done, _) = run(
        false,
        &[b"HTTP/1.1 200 OK\r\nContent-Length: 2, 2\r\n\r\nab"],
    );
    assert!(done);
    assert_eq!(body, b"ab");
    let mut r = ResponseReader::new(false);
    assert_eq!(
        r.feed(
            b"HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n",
            &mut |_| {}
        ),
        Err(HttpError::BadLength)
    );
}

#[test]
fn bad_status_and_huge_head() {
    let mut r = ResponseReader::new(false);
    assert_eq!(
        r.feed(b"HELLO\r\n\r\n", &mut |_| {}),
        Err(HttpError::BadStatus)
    );
    let mut r = ResponseReader::new(false);
    let big = vec![b'a'; MAX_HEAD + 10];
    assert_eq!(r.feed(&big, &mut |_| {}), Err(HttpError::HeadTooLarge));
}

#[test]
fn large_body_in_slices() {
    let n = 3 * 1024 * 1024u64;
    let mut r = ResponseReader::new(false);
    let mut got = 0u64;
    let head = alloc::format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {n}\r\n\r\n");
    r.feed(head.as_bytes(), &mut |_| {}).unwrap();
    let block = vec![7u8; 65536];
    while !r.is_done() {
        r.feed(&block, &mut |p| {
            if let Part::Body(b) = p {
                got += b.len() as u64
            }
        })
        .unwrap();
    }
    assert_eq!(got, n);
}

#[test]
fn header_helpers() {
    let h =
        b"HTTP/1.1 404 Not Found\r\nA: 1\r\nset-cookie: x=1\r\nSet-Cookie: y=2\r\nNoColon\r\n\r\n";
    assert_eq!(status_line(h), Some((404, &b"Not Found"[..])));
    assert_eq!(header(h, b"a"), Some(&b"1"[..]));
    assert_eq!(header_joined(h, b"SET-COOKIE").unwrap(), "x=1, y=2");
    assert_eq!(header_lines(h).count(), 3);
    assert!(has_token(b"keep-alive, Upgrade", b"upgrade"));
    assert_eq!(status_line(b"HTTP/1.1 2x0 OK\r\n"), None);
}

#[test]
fn content_range() {
    assert_eq!(
        parse_content_range(b"bytes 0-99/1000"),
        Some((0, 99, Some(1000)))
    );
    assert_eq!(parse_content_range(b"bytes 5-9/*"), Some((5, 9, None)));
    assert_eq!(parse_content_range(b"bytes 9-5/100"), None);
    assert_eq!(parse_content_range(b"bytes 0-100/100"), None);
    assert_eq!(parse_content_range(b"items 0-1/2"), None);
}

#[test]
fn target_parse() {
    let t = Target::parse("HTTPS://Example.COM:8443/a/b?x=1#frag").unwrap();
    assert!(t.https);
    assert_eq!(
        (t.host.as_str(), t.port, t.path.as_str()),
        ("example.com", 8443, "/a/b?x=1")
    );
    assert_eq!(t.origin(), "https://example.com:8443");
    assert_eq!(t.path_only(), "/a/b");
    let t = Target::parse("http://h?q").unwrap();
    assert_eq!((t.port, t.path.as_str()), (80, "/?q"));
    assert_eq!(t.origin(), "http://h");
    assert_eq!(t.to_url(), "http://h/?q");
    for bad in [
        "ftp://x/",
        "http://",
        "http://a b/",
        "http://u@h/",
        "http://h:0/",
        "http://h:70000/",
        "http://h/\r\nX: y",
        "http://.h/",
        "http://h..x/",
        "javascript:alert(1)",
        "http://é/",
    ] {
        assert_eq!(Target::parse(bad), None, "{bad}");
    }
    let long = alloc::format!("http://h/{}", "a".repeat(2000));
    assert_eq!(Target::parse(&long).unwrap().path.len(), 2001);
}

#[test]
fn resolve_references() {
    let base = Target::parse("https://a.com/x/y/z?q=1").unwrap();
    let r = |s: &str| resolve(&base, s).map(|t| t.to_url());
    assert_eq!(r("/p").as_deref(), Some("https://a.com/p"));
    assert_eq!(r("p").as_deref(), Some("https://a.com/x/y/p"));
    assert_eq!(r("../p?k").as_deref(), Some("https://a.com/x/p?k"));
    assert_eq!(r("../../../../p").as_deref(), Some("https://a.com/p"));
    assert_eq!(r("./").as_deref(), Some("https://a.com/x/y/"));
    assert_eq!(r("?n=2").as_deref(), Some("https://a.com/x/y/z?n=2"));
    assert_eq!(r("//b.org/k").as_deref(), Some("https://b.org/k"));
    assert_eq!(r("http://c.net").as_deref(), Some("http://c.net/"));
    assert_eq!(r("#top").as_deref(), Some("https://a.com/x/y/z?q=1"));
    assert_eq!(r("javascript:alert(1)"), None);
    assert_eq!(r("mailto:a@b"), None);
}

#[test]
fn request_is_written() {
    let t = Target::parse("http://h:8079/api?x=1").unwrap();
    let mut out = Vec::new();
    write_request(
        &mut out,
        &Request {
            method: "POST",
            target: &t,
            headers: &[
                ("Content-Type", "application/json"),
                ("Bad Name", "x"),
                ("X-Evil", "a\r\nInjected: 1"),
            ],
            body: Some(b"{}"),
            keep_alive: true,
        },
    );
    assert_eq!(
        out,
        b"POST /api?x=1 HTTP/1.1\r\nHost: h:8079\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\n{}"
    );
    let mut out = Vec::new();
    let t = Target::parse("https://h/").unwrap();
    write_request(
        &mut out,
        &Request {
            method: "PUT",
            target: &t,
            headers: &[],
            body: None,
            keep_alive: false,
        },
    );
    assert_eq!(
        out,
        b"PUT / HTTP/1.1\r\nHost: h\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    assert!(is_token("GET") && !is_token("G T") && !is_token(""));
}
