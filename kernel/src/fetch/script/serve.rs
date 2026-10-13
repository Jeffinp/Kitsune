//! One script request on the worker: same-origin rules, pre-flight, cookies, redirects, and the answer
//! streamed to the script process.

use super::*;
use crate::userproc::{PostError, page_try_post};
use kitsune_core::cors::{self, CorsError, Credentials, Mode};
use kitsune_core::http1::{self, Target, status_line};
use kitsune_core::web::script::{
    DATA_TAG, FetchHead, Redirect, ResponseType, data_message, end_message, error_message,
    head_message,
};

/// An empty body piece: the binary message header (`data_message`'s layout) ready for the bytes, so
/// the body is copied once into it and the message goes as it is.
fn new_piece(id: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(PIECE + 20 * 1024);
    v.push(DATA_TAG);
    v.extend_from_slice(&id.to_le_bytes());
    v
}
const PIECE_HEAD: usize = 5;

/// Most body bytes taken for one answer (the script process has 48 MiB for everything).
const MAX_BODY: usize = 64 * 1024 * 1024;
/// A piece is sent at least this often while data flows (ms), so a stream reads as it arrives.
const PIECE_MS: u64 = 40;

/// Hand `msg` to process `pid`, waiting while its queue is full. `false` when it is gone or the
/// request `id` was aborted meanwhile.
fn deliver(pid: u32, id: u32, mut msg: Vec<u8>) -> bool {
    let t0 = crate::netd::now_ms();
    let r = deliver_wait(pid, id, &mut msg);
    WAITED_MS.fetch_add(crate::netd::now_ms().saturating_sub(t0), Ordering::Relaxed);
    r
}

/// Time spent waiting for script processes to take their messages (ms; reported per request).
static WAITED_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn deliver_wait(pid: u32, id: u32, msg: &mut Vec<u8>) -> bool {
    loop {
        let m = core::mem::take(msg);
        match page_try_post(pid, m) {
            Ok(()) => return true,
            Err(PostError::Gone) => return false,
            Err(PostError::Full(m)) => *msg = m,
        }
        if aborted(pid, id) {
            return false;
        }
        crate::sched::block(crate::interrupts::ticks() + 1, || true);
    }
}

fn fail(pid: u32, id: u32, why: &str) {
    serial_println!("jsfetch: {} failed: {}", id, why);
    deliver(pid, id, error_message(id, why));
}

/// How the answer to one hop went.
enum Hop {
    /// Follow the redirect to this address with this status.
    Redirect(u16, String),
    /// Done (everything was sent, or the script went away).
    Done,
}

/// Serve `job` to its end.
pub(super) fn run(net: &mut netstack::Net, job: ScriptFetch) {
    let (pid, id) = (job.pid, job.req.id);
    let page = Target::parse(&job.page_url);
    let origin = page.as_ref().map_or(String::from("null"), Target::origin);
    let Some(mut url) = Target::parse(&job.req.url) else {
        return fail(pid, id, "invalid or unsupported URL");
    };
    let mut method = job.req.method.clone();
    let mut body = job.req.body.clone();
    let mut headers = job.req.headers.clone();
    let mut redirected = false;
    let t0 = crate::netd::now_ms();
    for hop in 0..=MAX_HOPS {
        if page.as_ref().is_some_and(|p| p.https) && !url.https {
            return fail(pid, id, "mixed content: an https page cannot fetch http");
        }
        let cross = url.origin() != origin;
        if cross && job.req.mode == Mode::SameOrigin {
            return fail(pid, id, CorsError::SameOriginOnly.message());
        }
        if cross
            && job.req.mode == Mode::NoCors
            && (!cors::is_simple_method(&method)
                || headers
                    .iter()
                    .any(|(n, v)| !cors::is_safelisted_header(n, v)))
        {
            return fail(pid, id, CorsError::NotSimple.message());
        }
        let send_cookies = job.req.credentials.send(!cross);
        if cross
            && job.req.mode == Mode::Cors
            && cors::needs_preflight(&method, &headers)
            && let Err(why) = preflight(net, &job, &url, &origin, &method, &headers)
        {
            return fail(pid, id, why);
        }
        let hop_result = exchange(
            net,
            &job,
            Exchange {
                url: &url,
                origin: &origin,
                page: page.as_ref(),
                method: &method,
                headers: &headers,
                body: body.as_deref(),
                cross,
                send_cookies,
                redirected,
                t0,
            },
        );
        match hop_result {
            Ok(Hop::Done) => return,
            Err(why) => return fail(pid, id, &why),
            Ok(Hop::Redirect(code, loc)) => {
                if job.req.redirect == Redirect::Error {
                    return fail(pid, id, "redirect not allowed (redirect: 'error')");
                }
                if hop == MAX_HOPS {
                    return fail(pid, id, "too many redirects");
                }
                let Some(next) = http1::resolve(&url, &loc) else {
                    return fail(pid, id, "bad redirect address");
                };
                serial_println!("jsfetch: {} redirect {} -> {}", id, code, next.to_url());
                if code == 303 || (matches!(code, 301 | 302) && method == "POST") {
                    if method != "HEAD" {
                        method = String::from("GET");
                    }
                    body = None;
                    headers.retain(|(n, _)| {
                        !matches!(
                            n.to_ascii_lowercase().as_str(),
                            "content-type" | "content-language" | "content-encoding"
                        )
                    });
                }
                url = next;
                redirected = true;
            }
        }
    }
}

/// The `OPTIONS` pre-flight of a cross-origin request that is not simple.
fn preflight(
    net: &mut netstack::Net,
    job: &ScriptFetch,
    url: &Target,
    origin: &str,
    method: &str,
    headers: &[(String, String)],
) -> Result<(), &'static str> {
    let names = cors::preflight_names(headers);
    let names_line = names.join(",");
    let mut h: Vec<(&str, &str)> = alloc::vec![
        ("User-Agent", http1::HTTP_USER_AGENT),
        ("Accept", "*/*"),
        ("Origin", origin),
        ("Access-Control-Request-Method", method),
    ];
    if !names_line.is_empty() {
        h.push(("Access-Control-Request-Headers", &names_line));
    }
    let mut head = Vec::new();
    let r = net.exchange(
        &netstack::Req {
            method: "OPTIONS",
            target: url,
            headers: &h,
            body: None,
            allow_insecure: allow_insecure(job, url),
        },
        &mut |ev| {
            if let netstack::Event::Head(x) = ev {
                head = x.to_vec();
            }
            true
        },
    );
    if r.is_err() || head.is_empty() {
        return Err("CORS pre-flight: network error");
    }
    let status = status_line(&head).map_or(0, |s| s.0);
    let include = job.req.credentials == Credentials::Include;
    match cors::check_preflight(origin, include, method, &names, status, &head) {
        Ok(()) => {
            serial_println!("cors: pre-flight ok for {} {}", method, url.to_url());
            Ok(())
        }
        Err(e) => {
            serial_println!("cors: pre-flight refused for {} {}", method, url.to_url());
            Err(e.message())
        }
    }
}

fn allow_insecure(job: &ScriptFetch, url: &Target) -> bool {
    url.https
        && !job.insecure_host.is_empty()
        && url.host.as_bytes().eq_ignore_ascii_case(&job.insecure_host)
}

/// One hop of a request.
struct Exchange<'a> {
    url: &'a Target,
    origin: &'a str,
    page: Option<&'a Target>,
    method: &'a str,
    headers: &'a [(String, String)],
    body: Option<&'a [u8]>,
    cross: bool,
    send_cookies: bool,
    redirected: bool,
    t0: u64,
}

/// Where the answer of a hop stands while it arrives.
struct Answer {
    /// A redirect to follow (its body is drained and dropped).
    redirect: Option<(u16, String)>,
    /// The script sees no headers and no body (no-cors answer from another origin).
    opaque: bool,
    /// The body is compressed: collected whole and inflated at the end.
    compressed: bool,
    piece: Vec<u8>,
    whole: Vec<u8>,
    total: usize,
    last_piece: u64,
    first_byte: u64,
    /// The script went away or aborted: stop.
    gone: bool,
    error: Option<String>,
    status: u16,
}

fn exchange(net: &mut netstack::Net, job: &ScriptFetch, x: Exchange) -> Result<Hop, String> {
    let (pid, id) = (job.pid, job.req.id);
    let url = x.url;
    let waited0 = WAITED_MS.load(Ordering::Relaxed);
    let safe = matches!(x.method, "GET" | "HEAD");
    let ctx = if x.send_cookies {
        cookies::context(
            url,
            cookies::Use::Subresource,
            safe,
            Some((&job.page_url, job.shield_on)),
        )
    } else {
        None
    };
    let cookie = ctx.map(|c| cookies::header_for(url, c)).unwrap_or_default();
    let referer = x
        .page
        .and_then(|p| cors::referrer(p, url))
        .unwrap_or_default();
    let lang = kitsune_core::browser::accept_language(kitsune_core::i18n::lang());
    let mut h: Vec<(&str, &str)> = alloc::vec![("User-Agent", http1::HTTP_USER_AGENT)];
    if !x
        .headers
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case("accept"))
    {
        h.push(("Accept", "*/*"));
    }
    if !x
        .headers
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case("accept-language"))
    {
        h.push(("Accept-Language", lang));
    }
    h.push(("Accept-Encoding", "identity"));
    for (n, v) in x.headers {
        h.push((n, v));
    }
    if x.cross || !safe {
        h.push(("Origin", x.origin));
    }
    if !referer.is_empty() {
        h.push(("Referer", &referer));
    }
    if !cookie.is_empty() {
        h.push(("Cookie", &cookie));
    }
    let include = job.req.credentials == Credentials::Include;
    let mut a = Answer {
        redirect: None,
        opaque: x.cross && job.req.mode == Mode::NoCors,
        compressed: false,
        piece: new_piece(id),
        whole: Vec::new(),
        total: 0,
        last_piece: 0,
        first_byte: 0,
        gone: false,
        error: None,
        status: 0,
    };
    serial_println!("jsfetch: {} {} {}", id, x.method, url.to_url());
    let r = net.exchange(
        &netstack::Req {
            method: x.method,
            target: url,
            headers: &h,
            body: x.body,
            allow_insecure: allow_insecure(job, url),
        },
        &mut |ev| match ev {
            netstack::Event::Raw(_) => !a.gone && a.error.is_none(),
            netstack::Event::Head(head) => on_head(job, &x, ctx, include, head, &mut a),
            netstack::Event::Body(b) => on_body(pid, id, b, &mut a),
        },
    );
    if let Some(e) = a.error {
        return Err(e);
    }
    if a.gone {
        serial_println!("jsfetch: {} aborted after {} bytes", id, a.total);
        return Ok(Hop::Done);
    }
    let done = match r {
        Ok(d) => d,
        Err(e) => return Err(alloc::format!("network error ({:?})", e)),
    };
    if let Some((code, loc)) = a.redirect {
        return Ok(Hop::Redirect(code, loc));
    }
    if !done.complete {
        return Err(String::from(
            "the connection closed before the answer ended",
        ));
    }
    if a.compressed {
        let mut raw = b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\n\r\n".to_vec();
        raw.extend_from_slice(&a.whole);
        a.whole = kitsune_core::browser::body_bytes(&raw).unwrap_or_default();
        for part in a.whole.chunks(PIECE) {
            if !deliver(pid, id, data_message(id, part)) {
                return Ok(Hop::Done);
            }
        }
    } else if a.piece.len() > PIECE_HEAD && !deliver(pid, id, core::mem::take(&mut a.piece)) {
        return Ok(Hop::Done);
    }
    deliver(pid, id, end_message(id));
    let now = crate::netd::now_ms();
    let body_ms = now.saturating_sub(a.first_byte.max(x.t0)).max(1);
    let (conns, handshakes, reuses) = netstack::pool_stats();
    serial_println!(
        "jsfetch: {} done: status {}, {} bytes in {} ms (first byte after {} ms, {} KB/s body, {} ms waiting for the page){} [net: {} connections, {} TLS handshakes, {} reuses]",
        id,
        a.status,
        a.total,
        now.saturating_sub(x.t0),
        a.first_byte.saturating_sub(x.t0),
        a.total as u64 * 1000 / 1024 / body_ms,
        WAITED_MS.load(Ordering::Relaxed).saturating_sub(waited0),
        if x.redirected { ", redirected" } else { "" },
        conns,
        handshakes,
        reuses
    );
    Ok(Hop::Done)
}

/// The head of a hop arrived: cookies, redirect, CORS, then the head message for the script.
fn on_head(
    job: &ScriptFetch,
    x: &Exchange,
    ctx: Option<kitsune_core::cookies::Context>,
    include: bool,
    head: &[u8],
    a: &mut Answer,
) -> bool {
    let (pid, id) = (job.pid, job.req.id);
    if let Some(c) = ctx {
        cookies::store(x.url, head, c);
    }
    let (status, reason) = status_line(head).unwrap_or((0, b""));
    a.status = status;
    if matches!(status, 301 | 302 | 303 | 307 | 308)
        && job.req.redirect != Redirect::Manual
        && let Some(loc) = http1::header(head, b"location")
    {
        a.redirect = Some((status, String::from_utf8_lossy(loc).into_owned()));
        return true;
    }
    let cors_mode = x.cross && job.req.mode == Mode::Cors;
    if cors_mode && let Err(e) = cors::check_response(x.origin, include, head) {
        serial_println!("cors: blocked {} (origin {})", x.url.to_url(), x.origin);
        a.error = Some(String::from(e.message()));
        return false;
    }
    let manual_redirect = matches!(status, 301 | 302 | 303 | 307 | 308);
    let kind = if manual_redirect && job.req.redirect == Redirect::Manual {
        ResponseType::OpaqueRedirect
    } else if a.opaque {
        ResponseType::Opaque
    } else if cors_mode {
        ResponseType::Cors
    } else {
        ResponseType::Basic
    };
    let hidden = matches!(kind, ResponseType::Opaque | ResponseType::OpaqueRedirect);
    a.opaque = hidden;
    let headers = if hidden {
        Vec::new()
    } else {
        cors::exposed_headers(head, cors_mode, include)
    };
    a.compressed = !hidden
        && http1::header(head, b"content-encoding").is_some_and(|v| {
            let v = v.trim_ascii();
            v.eq_ignore_ascii_case(b"gzip") || v.eq_ignore_ascii_case(b"deflate")
        });
    let cookie = cookies::document_get(&job.page_url);
    let url = x.url.to_url();
    let msg = head_message(&FetchHead {
        id,
        status: if hidden { 0 } else { status },
        status_text: if hidden {
            ""
        } else {
            core::str::from_utf8(reason).unwrap_or("")
        },
        url: if hidden { "" } else { &url },
        redirected: x.redirected,
        kind,
        headers: &headers,
        cookie: &cookie,
    });
    a.last_piece = crate::netd::now_ms();
    if !deliver(pid, id, msg) {
        a.gone = true;
        return false;
    }
    true
}

/// Body bytes of a hop arrived.
fn on_body(pid: u32, id: u32, b: &[u8], a: &mut Answer) -> bool {
    if a.redirect.is_some() || a.opaque {
        return true; // drained, never shown
    }
    if a.first_byte == 0 {
        a.first_byte = crate::netd::now_ms();
    }
    a.total += b.len();
    if a.total > MAX_BODY {
        a.error = Some(String::from("answer larger than 64 MiB"));
        return false;
    }
    if a.compressed {
        a.whole.extend_from_slice(b);
        return true;
    }
    a.piece.extend_from_slice(b);
    let now = crate::netd::now_ms();
    if a.piece.len() >= PIECE + PIECE_HEAD || now.saturating_sub(a.last_piece) >= PIECE_MS {
        a.last_piece = now;
        let msg = core::mem::replace(&mut a.piece, new_piece(id));
        if !deliver(pid, id, msg) {
            a.gone = true;
            return false;
        }
    }
    if aborted(pid, id) {
        a.gone = true;
        return false;
    }
    true
}
