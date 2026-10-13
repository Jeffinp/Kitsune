"""The network test routes of tools/w24-site.py (W55: fetch, XMLHttpRequest, cookies, CORS, streaming).

/net is a page whose script (/net/app.js) runs every check and prints `net: PASS name` or
`net: FAIL name ...` (console.log -> the guest's serial log), then `net: done P/T` and the title
"net: P/T". The other origin for CORS is the same host on the port just below the page's
(tools/w24-site.py listens on both). /net/speed only measures a big download (meant for https).

Routes: /net/json (JSON + Set-Cookie), /net/headers (the request headers as JSON), /net/echo (POST,
PUT: the body back as JSON), /net/big?n=BYTES (bytes i % 251, Range), /net/chunked (chunked),
/net/slow (5 s), /net/redirect (302 to /net/json), /net/conns (connections and requests seen),
/cors/open (Allow-Origin *), /cors/none (no CORS header), /cors/cred (exact origin + credentials),
/cors/put (pre-flight for PUT with X-Token).
"""
import json
import threading
import time

STATS = {"connections": 0, "requests": 0, "options": 0}
LOCK = threading.Lock()


def count(key):
    with LOCK:
        STATS[key] += 1


NET_PAGE = """<!doctype html><html><head><meta charset=utf-8><title>net: rodando</title>
<style>body{font-family:sans-serif;margin:16px} .pass{color:#0a7d2c} .fail{color:#b00020;font-weight:bold}</style>
</head><body><h1 id=h>Rede dos scripts</h1><p id=sum>rodando...</p><ul id=out></ul>
<script src="/net/app.js"></script></body></html>"""

SPEED_PAGE = """<!doctype html><html><head><meta charset=utf-8><title>net: velocidade</title></head>
<body><h1>Vazão</h1><p id=r>medindo...</p><script>
(async () => {
  const n = 8 * 1024 * 1024;
  for (const round of [1, 2]) {
    const t0 = performance.now();
    const r = await fetch('/net/big?n=' + n);
    const rd = r.body.getReader();
    let got = 0, pieces = 0;
    for (;;) { const { value, done } = await rd.read(); if (done) break; got += value.length; pieces++; }
    const ms = performance.now() - t0;
    const mbs = (got / 1048576) / (ms / 1000);
    console.log('net: speed round ' + round + ' ' + location.protocol + ' ' + got + ' bytes in ' + Math.round(ms) +
      ' ms = ' + mbs.toFixed(2) + ' MB/s (' + pieces + ' pieces)');
    document.getElementById('r').textContent += ' ' + mbs.toFixed(2) + ' MB/s';
  }
  console.log('net: speed done');
  document.title = 'net: velocidade medida';
})().catch((e) => console.log('net: speed FAIL ' + e));
</script></body></html>"""

NET_APP = r"""
'use strict';
const results = [];
const other = location.protocol + '//' + location.hostname + ':' + (Number(location.port || 80) - 1);
function log(ok, name, detail) {
  results.push(ok);
  console.log('net: ' + (ok ? 'PASS ' : 'FAIL ') + name + (detail ? ' ' + detail : ''));
  const li = document.createElement('li');
  li.className = ok ? 'pass' : 'fail';
  li.textContent = (ok ? 'ok ' : 'FALHOU ') + name + (detail ? ' (' + detail + ')' : '');
  document.getElementById('out').appendChild(li);
}
async function t(name, fn) {
  try { const d = await fn(); log(d !== false, name, typeof d === 'string' ? d : ''); }
  catch (e) { log(false, name, String(e)); }
}
function xhr(method, url, body, setup) {
  return new Promise((resolve, reject) => {
    const x = new XMLHttpRequest();
    x.open(method, url);
    if (setup) setup(x);
    x.onload = () => resolve(x);
    x.onerror = () => reject(new Error('xhr error'));
    x.send(body);
  });
}
(async () => {
  const c0 = await (await fetch('/net/conns')).json();
  await t('fetch json', async () => {
    const r = await fetch('/net/json');
    const j = await r.json();
    return r.ok && r.status === 200 && j.hello === 'mundo' && j.n === 42 &&
      r.headers.get('content-type') === 'application/json' && r.type === 'basic' || 'status ' + r.status;
  });
  await t('set-cookie and document.cookie', () => {
    const c = document.cookie;
    return c.includes('theme=dark') && !c.includes('sid=') || c;
  });
  document.cookie = 'js=1; Path=/';
  await t('cookies sent (HttpOnly too) and document.cookie write', async () => {
    const h = await (await fetch('/net/headers')).json();
    const c = h.cookie || '';
    return c.includes('sid=abc123') && c.includes('theme=dark') && c.includes('js=1') && document.cookie.includes('js=1') || 'cookie ' + c;
  });
  await t('POST json echo with Origin and Referer', async () => {
    const r = await fetch('/net/echo', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ a: 'ação' }) });
    const j = await r.json();
    return j.method === 'POST' && JSON.parse(j.body).a === 'ação' && j.type === 'application/json' &&
      j.origin === location.origin && j.referer === location.href || JSON.stringify(j);
  });
  await t('PUT with ArrayBuffer body', async () => {
    const j = await (await fetch('/net/echo', { method: 'PUT', body: new Uint8Array([1, 2, 3, 250]).buffer })).json();
    return j.method === 'PUT' && j.len === 4 || JSON.stringify(j);
  });
  await t('HEAD', async () => {
    const r = await fetch('/net/big?n=5000', { method: 'HEAD' });
    const b = await r.arrayBuffer();
    return r.status === 200 && r.headers.get('content-length') === '5000' && b.byteLength === 0 || 'status ' + r.status;
  });
  await t('XHR GET json', async () => {
    const x = await xhr('GET', '/net/json', null, (x) => { x.responseType = 'json'; });
    return x.status === 200 && x.response.n === 42 && x.getResponseHeader('Content-Type') === 'application/json' || x.status;
  });
  await t('XHR POST form', async () => {
    const x = await xhr('POST', '/net/echo', new URLSearchParams({ q: 'um dois', n: '3' }));
    const j = JSON.parse(x.responseText);
    return j.body === 'q=um+dois&n=3' && j.type.startsWith('application/x-www-form-urlencoded') || x.responseText;
  });
  await t('CORS allowed (Allow-Origin *)', async () => {
    const r = await fetch(other + '/cors/open');
    return r.type === 'cors' && (await r.text()) === 'open' && r.headers.get('x-hidden') === null || r.type;
  });
  await t('CORS refused without the header', async () => {
    try { await fetch(other + '/cors/none'); return 'it was readable'; }
    catch (e) { return e instanceof TypeError; }
  });
  await t('CORS with credentials', async () => {
    const j = await (await fetch(other + '/cors/cred', { credentials: 'include' })).json();
    return j.cookie.includes('sid=abc123') && j.origin === location.origin || JSON.stringify(j);
  });
  await t('CORS without credentials sends no cookie', async () => {
    const j = await (await fetch(other + '/cors/cred')).json();
    return j.cookie === '' || JSON.stringify(j);
  });
  await t('CORS pre-flight for PUT', async () => {
    const r = await fetch(other + '/cors/put', { method: 'PUT', headers: { 'X-Token': 'k1' }, body: 'x' });
    const j = await r.json();
    return j.method === 'PUT' && j.token === 'k1' && j.options >= 1 || JSON.stringify(j);
  });
  await t('no-cors is opaque', async () => {
    const r = await fetch(other + '/cors/none', { mode: 'no-cors' });
    return r.type === 'opaque' && r.status === 0 && (await r.text()) === '' || r.type;
  });
  await t('redirect followed', async () => {
    const r = await fetch('/net/redirect');
    return r.redirected && r.url.endsWith('/net/json') && (await r.json()).n === 42 || r.url;
  });
  await t('chunked body', async () => {
    const s = await (await fetch('/net/chunked')).text();
    return s === 'um-dois-três-quatro-cinco' || s;
  });
  await t('Range request', async () => {
    const r = await fetch('/net/big?n=1048576', { headers: { Range: 'bytes=1000-1999' } });
    const b = new Uint8Array(await r.arrayBuffer());
    return r.status === 206 && b.length === 1000 && b[0] === 1000 % 251 && b[999] === 1999 % 251 &&
      r.headers.get('content-range') === 'bytes 1000-1999/1048576' || 'status ' + r.status + ' len ' + b.length;
  });
  await t('streaming 4 MiB with getReader', async () => {
    const t0 = performance.now();
    const r = await fetch('/net/big?n=4194304');
    const rd = r.body.getReader();
    let got = 0, pieces = 0, bad = 0;
    for (;;) {
      const { value, done } = await rd.read();
      if (done) break;
      for (let i = 0; i < value.length; i += 4099) if (value[i] !== (got + i) % 251) bad++;
      got += value.length; pieces++;
    }
    const ms = performance.now() - t0;
    return got === 4194304 && bad === 0 && pieces > 1 ? 'pieces ' + pieces + ', ' + ((got / 1048576) / (ms / 1000)).toFixed(2) + ' MB/s' : false;
  });
  await t('8 MiB arrayBuffer', async () => {
    const t0 = performance.now();
    const b = new Uint8Array(await (await fetch('/net/big?n=8388608')).arrayBuffer());
    const ms = performance.now() - t0;
    return b.length === 8388608 && b[8388607] === 8388607 % 251 ? ((b.length / 1048576) / (ms / 1000)).toFixed(2) + ' MB/s' : 'len ' + b.length;
  });
  await t('abort with AbortController', async () => {
    const ac = new AbortController();
    setTimeout(() => ac.abort(), 300);
    try { await fetch('/net/slow', { signal: ac.signal }); return 'not aborted'; }
    catch (e) { return e.name === 'AbortError' || e.name; }
  });
  await t('blocked by the ad blocker', async () => {
    try { await fetch('/ads/banner-728.png'); return 'it was fetched'; }
    catch (e) { return e instanceof TypeError && /ad blocker/.test(e.message) || String(e); }
  });
  await t('small APIs', () => {
    const enc = new TextEncoder().encode('ação €');
    const ok = new TextDecoder().decode(enc) === 'ação €' && enc.length === 10 &&
      crypto.getRandomValues(new Uint32Array(4)).some((x) => x !== 0) && crypto.randomUUID().length === 36 &&
      structuredClone({ m: new Map([[1, [2]]]) }).m.get(1)[0] === 2 && atob(btoa('xyz')) === 'xyz' &&
      new Blob(['ab', new Uint8Array([99])]).size === 3 && typeof performance.now() === 'number' &&
      navigator.sendBeacon('/net/echo', 'beacon') === true;
    return ok;
  });
  await t('keep-alive (connections reused)', async () => {
    const c1 = await (await fetch('/net/conns')).json();
    const conns = c1.connections - c0.connections, reqs = c1.requests - c0.requests;
    return conns < reqs / 2 ? conns + ' connections for ' + reqs + ' requests' : 'connections ' + conns + ' requests ' + reqs;
  });
  const pass = results.filter((x) => x).length;
  document.getElementById('sum').textContent = pass + ' de ' + results.length + ' verificações passaram';
  document.title = 'net: ' + pass + '/' + results.length;
  console.log('net: done ' + pass + '/' + results.length);
})();
"""


def big(n, start=0, end=None):
    """Bytes i % 251 for i in [start, end)."""
    end = n if end is None else end
    pat = bytes(i % 251 for i in range(251))
    first = start % 251
    out = bytearray()
    out += pat[first:]
    while len(out) < end - start:
        out += pat
    return bytes(out[: end - start])


def handle(h, method, u, q):
    """Answer a /net or /cors route on handler `h`; False when the path is not one of them."""
    p = u.path
    origin = h.headers.get("Origin")
    if p == "/net" and method == "GET":
        h.reply(200, "text/html; charset=utf-8", NET_PAGE.encode())
    elif p == "/net/speed" and method == "GET":
        h.reply(200, "text/html; charset=utf-8", SPEED_PAGE.encode())
    elif p == "/net/app.js":
        h.reply(200, "text/javascript; charset=utf-8", NET_APP.encode())
    elif p == "/net/json":
        body = json.dumps({"hello": "mundo", "n": 42}).encode()
        h.reply(200, "application/json", body, [
            ("Set-Cookie", "sid=abc123; Path=/; HttpOnly; SameSite=Lax"),
            ("Set-Cookie", "theme=dark; Path=/; Max-Age=3600"),
        ])
    elif p == "/net/headers":
        hd = {k.lower(): v for k, v in h.headers.items()}
        h.reply(200, "application/json", json.dumps(hd).encode())
    elif p == "/net/echo":
        n = int(h.headers.get("Content-Length") or 0)
        data = h.rfile.read(n) if n else b""
        body = json.dumps({
            "method": method, "len": len(data), "body": data.decode("utf-8", "replace"),
            "type": h.headers.get("Content-Type", ""), "origin": h.headers.get("Origin", ""),
            "referer": h.headers.get("Referer", ""),
        }).encode()
        h.reply(200, "application/json", body)
    elif p == "/net/big":
        n = int(q.get("n", ["1048576"])[0])
        rng = h.headers.get("Range", "")
        if rng.startswith("bytes="):
            a, _, b = rng[6:].partition("-")
            a = int(a)
            b = min(int(b) if b else n - 1, n - 1)
            data = b"" if method == "HEAD" else big(n, a, b + 1)
            h.reply(206, "application/octet-stream", data, [("Content-Range", f"bytes {a}-{b}/{n}")],
                    length=b - a + 1)
        else:
            data = b"" if method == "HEAD" else big(n)
            h.reply(200, "application/octet-stream", data, [("Accept-Ranges", "bytes")], length=n)
    elif p == "/net/chunked":
        h.send_response(200)
        h.send_header("Content-Type", "text/plain; charset=utf-8")
        h.send_header("Transfer-Encoding", "chunked")
        h.end_headers()
        for i, part in enumerate(["um-", "dois-", "três-", "quatro-", "cinco"]):
            b = part.encode()
            h.wfile.write(f"{len(b):X}\r\n".encode() + b + b"\r\n")
            h.wfile.flush()
            time.sleep(0.05 * i)
        h.wfile.write(b"0\r\n\r\n")
    elif p == "/net/slow":
        time.sleep(5)
        h.reply(200, "text/plain", b"slow")
    elif p == "/net/redirect":
        h.reply(302, "text/plain", b"", [("Location", "/net/json")])
    elif p == "/net/conns":
        with LOCK:
            body = json.dumps(STATS).encode()
        h.reply(200, "application/json", body)
    elif p == "/cors/open":
        h.reply(200, "text/plain", b"open", [("Access-Control-Allow-Origin", "*"), ("X-Hidden", "1")])
    elif p == "/cors/none":
        h.reply(200, "text/plain", b"secret")
    elif p == "/cors/cred":
        body = json.dumps({"cookie": h.headers.get("Cookie", ""), "origin": origin or ""}).encode()
        h.reply(200, "application/json", body, [
            ("Access-Control-Allow-Origin", origin or "*"),
            ("Access-Control-Allow-Credentials", "true"), ("Vary", "Origin"),
        ])
    elif p == "/cors/put":
        if method == "OPTIONS":
            count("options")
            h.reply(204, "text/plain", b"", [
                ("Access-Control-Allow-Origin", origin or "*"),
                ("Access-Control-Allow-Methods", "GET, PUT"),
                ("Access-Control-Allow-Headers", "X-Token, Content-Type"),
                ("Access-Control-Max-Age", "0"),
            ])
        else:
            n = int(h.headers.get("Content-Length") or 0)
            if n:
                h.rfile.read(n)
            body = json.dumps({"method": method, "token": h.headers.get("X-Token", ""),
                               "options": STATS["options"]}).encode()
            h.reply(200, "application/json", body, [("Access-Control-Allow-Origin", origin or "*")])
    else:
        return False
    return True
