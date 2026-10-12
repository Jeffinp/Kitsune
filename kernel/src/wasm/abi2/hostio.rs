//! Host work on paths and the network: the descriptor results, escapes report and `http_get`.

use super::*;

pub(super) fn net_http_get(g: &mut dyn Guest, url: i32, ulen: i32, out: i32, cap: i32) -> R<i32> {
    let bytes = match read_capped(g, url, ulen, MAX_URL)? {
        Ok(b) => b,
        Err(e) => return Ok(e),
    };
    range(g, out, cap.clamp(0, appnet::MAX_BODY as i32))?;
    let st = v2(g)?;
    // One gate for the request (and, on the fetcher, for every redirect hop):
    // permission, destination filter, the manifest's allow-list.
    let parsed = match appnet::authorize(st.net, &st.net_hosts, &bytes) {
        Ok(u) => u,
        Err(code) => return Ok(code),
    };
    if !st.limiter.allow(mono_ms()) {
        return Ok(ERR_NET);
    }
    crate::serial_println!(
        "[app {}] net_http_get {}://{}:{}{}",
        st.id,
        if parsed.https { "https" } else { "http" },
        parsed.host,
        parsed.port,
        parsed.path
    );
    let (perm, hosts, id) = (st.net, st.net_hosts.clone(), st.id.clone());
    // The transport is the fetcher thread (`fetch`, owned by netd): this thread sleeps until the
    // answer, a timeout or the death of the fetcher, and holds no lock meanwhile. HTTPS gets a
    // longer budget (a software TLS handshake). The other apps wait for this call (one `appd`
    // thread); the compositor and everything else keep running.
    let budget = if parsed.https {
        appnet::TIMEOUT_HTTPS_MS
    } else {
        appnet::TIMEOUT_MS
    };
    let cap = cap.clamp(0, appnet::MAX_BODY as i32) as usize;
    let loaded = match crate::fetch::app_get(&bytes, perm, &hosts, budget) {
        Ok(l) => l,
        Err(e) => {
            crate::serial_println!("[app {}] net_http_get failed: {:?}", id, e);
            return Ok(ERR_NET);
        }
    };
    let (body, cut) = match appnet::app_response(&loaded.data, cap) {
        Ok(r) => r,
        Err(e) => {
            crate::serial_println!("[app {}] net_http_get: response refused: {:?}", id, e);
            return Ok(e.code());
        }
    };
    write_guest(g, out, &body)?;
    g.charge(500 + body.len() as u64 / 16)?;
    crate::serial_println!(
        "[app {}] net_http_get: {} bytes{}",
        id,
        body.len(),
        if cut || loaded.truncated {
            " (cut)"
        } else {
            ""
        }
    );
    Ok(body.len() as i32)
}

pub(super) fn res(r: Result<i32, kitsune_core::appfs::FsError>) -> i32 {
    match r {
        Ok(v) => v,
        Err(e) => e.code(),
    }
}

/// Logs path-escape attempts once each (the sandbox counts them).
pub(super) fn report_escapes(st: &mut V2) {
    let n = st.sandbox.escapes;
    if n > st.escapes_logged {
        crate::serial_println!(
            "[app {}] sandbox: path escape refused (attempt {})",
            st.id,
            n
        );
        st.escapes_logged = n;
    }
}

pub(super) fn path_op(
    g: &mut dyn Guest,
    p: i32,
    n: i32,
    f: impl FnOnce(
        &mut Sandbox,
        &mut dyn kitsune_core::appfs::AppFs,
        &[u8],
    ) -> Result<(), kitsune_core::appfs::FsError>,
) -> R<i32> {
    let path = match read_capped(g, p, n, MAX_PATH_ARG)? {
        Ok(b) => b,
        Err(e) => return Ok(e),
    };
    let st = v2(g)?;
    let r = appfs_backend::try_with(|fs| f(&mut st.sandbox, fs, &path));
    report_escapes(st);
    Ok(res(r.map(|()| 0)))
}
