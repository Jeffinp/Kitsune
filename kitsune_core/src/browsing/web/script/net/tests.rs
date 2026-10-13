use super::*;
use alloc::vec;

fn msg(s: &str) -> Vec<u8> {
    s.as_bytes().to_vec()
}

#[test]
fn a_request_is_read() {
    let m = msg(
        "7\npost\nhttps://api.example.com/x?y=1\ncors\ninclude\nfollow\n1\nContent-Type: application/json\nX-A:  b \nCookie: stolen=1\nHost: evil\nBad Name: 1\n\n{\"k\":1}",
    );
    let r = parse_fetch(&m).unwrap();
    assert_eq!(r.id, 7);
    assert_eq!(r.method, "POST");
    assert_eq!(r.url, "https://api.example.com/x?y=1");
    assert_eq!(
        (r.mode, r.credentials, r.redirect),
        (Mode::Cors, Credentials::Include, Redirect::Follow)
    );
    assert_eq!(
        r.headers,
        vec![
            (
                String::from("Content-Type"),
                String::from("application/json")
            ),
            (String::from("X-A"), String::from("b"))
        ]
    );
    assert_eq!(r.body.as_deref(), Some(&b"{\"k\":1}"[..]));
}

#[test]
fn binary_body_and_defaults() {
    let mut m = msg("1\nPUT\nhttp://h/\n\n\n\n1\n\n");
    m.extend_from_slice(&[0, 255, 10, 13]);
    let r = parse_fetch(&m).unwrap();
    assert_eq!(
        (r.mode, r.credentials, r.redirect),
        (Mode::Cors, Credentials::SameOrigin, Redirect::Follow)
    );
    assert_eq!(r.body.as_deref(), Some(&[0u8, 255, 10, 13][..]));
    // A GET never has a body; a custom method keeps its case.
    let r = parse_fetch(&msg(
        "2\nget\nhttp://h/\nno-cors\nomit\nmanual\n1\n\nignored",
    ))
    .unwrap();
    assert_eq!((r.method.as_str(), r.body), ("GET", None));
    let r = parse_fetch(&msg("3\nPatch\nhttp://h/\ncors\nomit\nerror\n0\n\n")).unwrap();
    assert_eq!(r.method, "Patch");
}

#[test]
fn bad_requests_are_refused() {
    for bad in [
        "",
        "x\nGET\nhttp://h/\ncors\nomit\nfollow\n0\n\n",
        "1\nCONNECT\nhttp://h/\ncors\nomit\nfollow\n0\n\n",
        "1\ntrace\nhttp://h/\ncors\nomit\nfollow\n0\n\n",
        "1\nG T\nhttp://h/\ncors\nomit\nfollow\n0\n\n",
        "1\nGET\nhttp://h/\nweird\nomit\nfollow\n0\n\n",
        "1\nGET\nhttp://h/\ncors\nomit\nfollow\n0\nA: b",
    ] {
        assert_eq!(parse_fetch(bad.as_bytes()), None, "{bad:?}");
    }
    let long = alloc::format!(
        "1\nGET\nhttp://h/{}\ncors\nomit\nfollow\n0\n\n",
        "a".repeat(9000)
    );
    assert_eq!(parse_fetch(long.as_bytes()), None);
    // At most MAX_FETCH_HEADERS headers are kept.
    let mut many = String::from("1\nGET\nhttp://h/\ncors\nomit\nfollow\n0\n");
    for i in 0..100 {
        many.push_str(&alloc::format!("X-{i}: v\n"));
    }
    many.push('\n');
    assert_eq!(
        parse_fetch(many.as_bytes()).unwrap().headers.len(),
        MAX_FETCH_HEADERS
    );
}

#[test]
fn messages_from_the_script() {
    let m = msg("F5\nGET\nhttp://h/\ncors\nomit\nfollow\n0\n\n");
    assert!(matches!(parse_message(&m), FromScript::Fetch(r) if r.id == 5));
    assert_eq!(parse_message(b"X12"), FromScript::Abort(12));
    assert_eq!(parse_message(b"Xx"), FromScript::Unknown);
    assert_eq!(
        parse_message(b"Ca=1; Path=/"),
        FromScript::SetCookie("a=1; Path=/")
    );
}

#[test]
fn messages_to_the_script() {
    let headers = vec![(String::from("content-type"), String::from("text/plain"))];
    let h = head_message(&FetchHead {
        id: 3,
        status: 206,
        status_text: "Partial \"Content\"",
        url: "http://h/x",
        redirected: true,
        kind: ResponseType::Cors,
        headers: &headers,
        cookie: "a=1",
    });
    assert_eq!(
        core::str::from_utf8(&h).unwrap(),
        r#"{"t":"fetch","id":3,"ev":"head","status":206,"statusText":"Partial \"Content\"","url":"http://h/x","redirected":true,"type":"cors","cookie":"a=1","headers":[["content-type","text/plain"]]}"#
    );
    assert_eq!(data_message(258, b"ab"), vec![1, 2, 1, 0, 0, b'a', b'b']);
    assert_eq!(
        end_message(9),
        br#"{"t":"fetch","id":9,"ev":"end"}"#.to_vec()
    );
    assert_eq!(
        error_message(9, "x\ny"),
        br#"{"t":"fetch","id":9,"ev":"error","message":"x\ny"}"#.to_vec()
    );
    assert_eq!(
        cookie_message("a=\"1\""),
        br#"{"t":"cookie","v":"a=\"1\""}"#.to_vec()
    );
}
