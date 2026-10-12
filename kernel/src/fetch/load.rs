//! Loading one URL on the worker: a page (with redirects and TLS), a style sheet or an image.

use super::*;

/// Resolve a URL and fetch it (HTTP or HTTPS), following up to
/// [`kitsune_core::redirect::MAX_REDIRECTS`] redirects. Returns the final page,
/// or why the navigation failed. Redirect policy (scheme kept, https -> http
/// refused, loops, bad `Location` values) lives in `kitsune_core::redirect`.
pub(super) fn fetch_url(
    net: &mut netstack::Net,
    url: &[u8],
    insecure_host: &[u8],
    cap: usize,
    app: Option<&AppPolicy>,
    body: Option<&[u8]>,
) -> FetchResult {
    use kitsune_core::browser::{header_value, parse_url, status_code};
    use kitsune_core::redirect::Redirects;
    let mut cur: Vec<u8> = url.to_vec();
    let mut chain: Option<Redirects> = None;
    // The form data of a POST: kept through 307/308, dropped (the next hop is a GET) otherwise.
    let mut body = body;

    loop {
        // An app's request, and each redirect hop it is sent on to, passes the same gate:
        // permission, destination filter and allow-list (`kitsune_core::appnet`).
        if let Some(p) = app
            && let Err(code) = kitsune_core::appnet::authorize(p.perm, &p.hosts, &cur)
        {
            serial_println!("fetch: app request refused ({})", code);
            return Err(FailReason::Network);
        }
        let u = parse_url(&cur).ok_or(FailReason::Network)?;
        let chain = chain.get_or_insert_with(|| Redirects::new(&u));
        let (Ok(host), Ok(path)) = (
            core::str::from_utf8(u.host()),
            core::str::from_utf8(u.path()),
        ) else {
            return Err(FailReason::Network);
        };
        serial_println!(
            "fetch: {} {}://{}{} :{}",
            if body.is_some() { "POST" } else { "GET" },
            if u.https { "https" } else { "http" },
            host,
            path,
            u.port
        );
        let (r, conn) = if u.https {
            // Certificate dates need a confirmed clock: if the boot-time SNTP did not
            // succeed, try again (at most once a minute) before the handshake.
            resync_clock_if_needed(net);
            // Validation is skipped only for the one host the user allowed.
            // (Never for an app: `insecure_host` is empty for them.)
            let allow = app.is_none()
                && !insecure_host.is_empty()
                && u.host().eq_ignore_ascii_case(insecure_host);
            net.https_get(host, path, u.port, allow, cap, body)?
        } else {
            (net.http_get(host, path, u.port, cap, body)?, Conn::Plain)
        };

        let code = status_code(&r.data).unwrap_or(0);
        if matches!(code, 301 | 302 | 303 | 307 | 308)
            && let Some(loc) = header_value(&r.data, b"location")
        {
            match chain.follow(&u, loc) {
                Ok(next) => {
                    cur = next;
                    if !matches!(code, 307 | 308) {
                        body = None;
                    }
                    serial_println!("fetch: {} redirect", code);
                    continue;
                }
                Err(e) => {
                    serial_println!("fetch: {} redirect refused: {:?}", code, e);
                    return Err(FailReason::from_redirect(e));
                }
            }
        }

        serial_println!(
            "fetch: {} bytes (status {}){}",
            r.data.len(),
            code,
            if r.truncated { " truncated" } else { "" }
        );
        return Ok(Loaded {
            data: r.data,
            conn,
            truncated: r.truncated,
            cert: r.cert,
        });
    }
}

/// Download one picture (same stack, same redirect rules; at most `MAX_IMAGE_BYTES`) and turn it
/// into column-sized pixels here, on the worker, so the compositor never decodes.
/// Download one style sheet: its decoded body when the answer is a 200, else `None`.
pub(super) fn fetch_sheet(
    net: &mut netstack::Net,
    url: &[u8],
    insecure_host: &[u8],
) -> Option<Vec<u8>> {
    let r = fetch_url(net, url, insecure_host, SHEET_CAP, None, None).ok()?;
    let name = core::str::from_utf8(url).unwrap_or("?");
    let code = kitsune_core::browser::status_code(&r.data).unwrap_or(0);
    if code != 200 {
        serial_println!("css: status {} for {}", code, name);
        return None;
    }
    let body = kitsune_core::browser::page_body_partial(&r.data, r.truncated).body;
    serial_println!("css: {} -> {} bytes", name, body.len());
    Some(body)
}

pub(super) fn fetch_image(
    net: &mut netstack::Net,
    url: &[u8],
    insecure_host: &[u8],
    fit_w: usize,
) -> Result<ImgLoaded, ImgFail> {
    let r = match fetch_url(
        net,
        url,
        insecure_host,
        imgcache::MAX_IMAGE_BYTES + 8 * 1024,
        None,
        None,
    ) {
        Ok(r) => r,
        Err(_) => return Err(ImgFail::Failed),
    };
    let code = kitsune_core::browser::status_code(&r.data).unwrap_or(0);
    let name = core::str::from_utf8(url).unwrap_or("?");
    if code != 200 {
        serial_println!("img: status {} for {}", code, name);
        return Err(ImgFail::Failed);
    }
    if r.truncated {
        return Err(ImgFail::TooBig);
    }
    let body = kitsune_core::browser::page_body(&r.data);
    drop(r);
    let out = imgcache::decode_for_page(&body, fit_w);
    match &out {
        Ok(l) => serial_println!(
            "img: {} -> {}x{} (shown {}x{})",
            name,
            l.orig_w,
            l.orig_h,
            l.img.width(),
            l.img.height()
        ),
        Err(e) => serial_println!("img: {} failed: {:?}", name, e),
    }
    out
}
