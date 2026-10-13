//! The HTTP request the browser sends: `Accept-Language`, GET and POST.

use super::*;

/// The `Accept-Language` value the browser sends: the language of the interface first, then
/// English (`pt-BR,pt;q=0.9,en;q=0.8` or `en;q=1`). It is a catalog entry, so a new language
/// brings its own.
pub fn accept_language(lang: Lang) -> &'static str {
    i18n::tr_in(lang, tk!("web.accept_language"))
}

/// Append an HTTP/1.1 `GET` request to `req` (`Connection: close`: one request per
/// connection; shared by the plain and the TLS paths), asking for the page in `lang`.
pub fn build_get_request(
    req: &mut Vec<u8>,
    lang: Lang,
    host: &str,
    path: &str,
    port: u16,
    tls: bool,
) {
    build_request(req, lang, host, path, port, tls, None);
}

/// Append an HTTP/1.1 request to `req`: a `GET`, or a `POST` of `body` as
/// `application/x-www-form-urlencoded` when there is one.
pub fn build_request(
    req: &mut Vec<u8>,
    lang: Lang,
    host: &str,
    path: &str,
    port: u16,
    tls: bool,
    body: Option<&[u8]>,
) {
    req.extend_from_slice(if body.is_some() { b"POST " } else { b"GET " });
    req.extend_from_slice(path.as_bytes());
    req.extend_from_slice(b" HTTP/1.1\r\nHost: ");
    req.extend_from_slice(host.as_bytes());
    let default_port = if tls { 443 } else { 80 };
    if port != default_port {
        req.push(b':');
        let mut digits = [0u8; 5];
        let mut n = port;
        let mut i = digits.len();
        loop {
            i -= 1;
            digits[i] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        req.extend_from_slice(&digits[i..]);
    }
    req.extend_from_slice(
        concat!(
            "\r\nUser-Agent: Mozilla/5.0 (X11; Kitsune x86_64) Kitsune/",
            env!("CARGO_PKG_VERSION"),
            "\r\nAccept: text/html, image/png, image/bmp, */*;q=0.1\r\nAccept-Language: "
        )
        .as_bytes(),
    );
    req.extend_from_slice(accept_language(lang).as_bytes());
    req.extend_from_slice(b"\r\nAccept-Encoding: gzip, deflate\r\nConnection: close\r\n");
    if let Some(body) = body {
        req.extend_from_slice(
            b"Content-Type: application/x-www-form-urlencoded\r\nContent-Length: ",
        );
        let mut digits = [0u8; 20];
        let mut n = body.len();
        let mut i = digits.len();
        loop {
            i -= 1;
            digits[i] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        req.extend_from_slice(&digits[i..]);
        req.extend_from_slice(b"\r\n");
    }
    req.extend_from_slice(b"\r\n");
    if let Some(body) = body {
        req.extend_from_slice(body);
    }
}
