use super::*;
use alloc::string::ToString;
use alloc::vec;

fn h(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

const O: &str = "http://203.0.113.5:8079";

#[test]
fn simple_requests_need_no_preflight() {
    assert!(!needs_preflight("GET", &[]));
    assert!(!needs_preflight(
        "POST",
        &h(&[("Content-Type", "text/plain;charset=UTF-8")])
    ));
    assert!(!needs_preflight(
        "GET",
        &h(&[("Accept", "application/json"), ("Range", "bytes=0-99")])
    ));
    assert!(needs_preflight("PUT", &[]));
    assert!(needs_preflight("DELETE", &[]));
    assert!(needs_preflight(
        "POST",
        &h(&[("Content-Type", "application/json")])
    ));
    assert!(needs_preflight("GET", &h(&[("X-Token", "1")])));
    assert!(needs_preflight("GET", &h(&[("Range", "bytes=-500")])));
    assert!(needs_preflight("GET", &h(&[("Range", "bytes=0-1,5-9")])));
    assert!(needs_preflight("GET", &h(&[("Accept", "a(b)")])));
    assert_eq!(
        preflight_names(&h(&[
            ("X-B", "1"),
            ("x-a", "2"),
            ("X-B", "3"),
            ("Accept", "*/*")
        ])),
        vec!["x-a".to_string(), "x-b".to_string()]
    );
}

#[test]
fn forbidden_headers() {
    for n in [
        "Cookie",
        "Host",
        "origin",
        "Referer",
        "Content-Length",
        "Sec-Fetch-Mode",
        "Proxy-Authorization",
        "Connection",
    ] {
        assert!(is_forbidden_header(n), "{n}");
    }
    for n in ["Range", "Content-Type", "X-Requested-With", "Authorization"] {
        assert!(!is_forbidden_header(n), "{n}");
    }
}

#[test]
fn allow_origin() {
    let head = |acao: &str, acac: &str| {
        let mut s = String::from("HTTP/1.1 200 OK\r\n");
        if !acao.is_empty() {
            s.push_str(&alloc::format!("Access-Control-Allow-Origin: {acao}\r\n"));
        }
        if !acac.is_empty() {
            s.push_str(&alloc::format!(
                "Access-Control-Allow-Credentials: {acac}\r\n"
            ));
        }
        s.push_str("\r\n");
        s
    };
    assert_eq!(
        check_response(O, false, head("", "").as_bytes()),
        Err(CorsError::AllowOrigin)
    );
    assert_eq!(check_response(O, false, head("*", "").as_bytes()), Ok(()));
    assert_eq!(
        check_response(O, true, head("*", "true").as_bytes()),
        Err(CorsError::Credentials)
    );
    assert_eq!(check_response(O, false, head(O, "").as_bytes()), Ok(()));
    assert_eq!(
        check_response(O, true, head(O, "").as_bytes()),
        Err(CorsError::Credentials)
    );
    assert_eq!(check_response(O, true, head(O, "true").as_bytes()), Ok(()));
    assert_eq!(
        check_response(O, false, head("http://evil.example", "").as_bytes()),
        Err(CorsError::AllowOrigin)
    );
    assert_eq!(
        check_response(O, false, head("http://203.0.113.5:8079/", "").as_bytes()),
        Err(CorsError::AllowOrigin)
    );
}

#[test]
fn preflight() {
    let ok = alloc::format!(
        "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: {O}\r\nAccess-Control-Allow-Methods: GET, PUT\r\nAccess-Control-Allow-Headers: X-Token, content-type\r\n\r\n"
    );
    let names = vec!["content-type".to_string(), "x-token".to_string()];
    assert_eq!(
        check_preflight(O, false, "PUT", &names, 204, ok.as_bytes()),
        Ok(())
    );
    assert_eq!(
        check_preflight(O, false, "DELETE", &names, 204, ok.as_bytes()),
        Err(CorsError::Preflight)
    );
    assert_eq!(
        check_preflight(
            O,
            false,
            "PUT",
            &["x-other".to_string()],
            204,
            ok.as_bytes()
        ),
        Err(CorsError::Preflight)
    );
    assert_eq!(
        check_preflight(O, false, "PUT", &names, 403, ok.as_bytes()),
        Err(CorsError::Preflight)
    );
    let star = "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: *\r\nAccess-Control-Allow-Headers: *\r\n\r\n";
    assert_eq!(
        check_preflight(O, false, "PATCH", &names, 200, star.as_bytes()),
        Ok(())
    );
    assert_eq!(
        check_preflight(
            O,
            false,
            "GET",
            &["authorization".to_string()],
            200,
            star.as_bytes()
        ),
        Err(CorsError::Preflight)
    );
    assert_eq!(
        check_preflight(O, true, "PATCH", &names, 200, star.as_bytes()),
        Err(CorsError::Preflight)
    );
}

#[test]
fn exposed() {
    let head = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nX-Secret: 1\r\nX-Shown: 2\r\nSet-Cookie: a=1\r\nAccess-Control-Expose-Headers: x-shown\r\n\r\n";
    let names = |v: Vec<(String, String)>| v.into_iter().map(|(n, _)| n).collect::<Vec<_>>();
    assert_eq!(
        names(exposed_headers(head, false, false)),
        [
            "content-type",
            "x-secret",
            "x-shown",
            "access-control-expose-headers"
        ]
    );
    assert_eq!(
        names(exposed_headers(head, true, false)),
        ["content-type", "x-shown"]
    );
}

#[test]
fn referrer_policy() {
    let page = Target::parse("https://a.com/p/q?x#f").unwrap();
    let same = Target::parse("https://a.com/api").unwrap();
    let other = Target::parse("https://b.com/api").unwrap();
    let down = Target::parse("http://a.com/api").unwrap();
    assert_eq!(
        referrer(&page, &same).as_deref(),
        Some("https://a.com/p/q?x")
    );
    assert_eq!(referrer(&page, &other).as_deref(), Some("https://a.com/"));
    assert_eq!(referrer(&page, &down), None);
}

#[test]
fn credentials_modes() {
    assert!(!Credentials::Omit.send(true));
    assert!(Credentials::SameOrigin.send(true));
    assert!(!Credentials::SameOrigin.send(false));
    assert!(Credentials::Include.send(false));
}

#[test]
fn javascript_types_for_inserted_scripts() {
    for ok in [
        &b"text/javascript"[..],
        b"application/javascript; charset=utf-8",
        b" Text/JavaScript ",
        b"application/x-javascript",
        b"text/ecmascript",
    ] {
        assert!(is_javascript_type(ok), "{:?}", ok);
    }
    for no in [
        &b""[..],
        b"text/html",
        b"application/json",
        b"text/plain",
        b"image/png",
        b"application/javascriptx",
        b"text/javascript-x",
    ] {
        assert!(!is_javascript_type(no), "{:?}", no);
    }
}
