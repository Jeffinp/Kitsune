// Host probe of a real page's JavaScript with the Kitsune page prelude (user-apps/jsrun/src/bin/jsrun/*.js,
// in the order jsrun.rs loads them), with Node playing the browser: the page and its scripts come from
// the network (curl, through the sandbox proxy when there is one), the scripts' fetch/XHR go out for
// real (one at a time, like the fetcher, with a small cookie jar and the CORS checks), the media
// elements get a fake decoder that reads the WebM timestamps of what is appended, and every element gets
// a layout box. Made for bringing up YouTube (docs/design/tls-browser.md §16), useful for any page.
//
//   node tools/yt-probe.js [URL] [--engine qjs|node] [--secs N] [--cache DIR] [--play] [-v] [--net-log]
//
// --engine qjs (the default) runs the real page process: QuickJS with glue.c and page.c, built for the
// host from tools/jsrun-host.c (needs clang), talking over pipes; its times are native ones (the QEMU
// TCG guest is ~20-50x slower). --engine node runs the prelude in a Node vm (V8: faster to iterate, but
// another engine). Prints the console of the page, uncaught errors, the requests made (method, URL,
// status, bytes, ms), the scripts that ran long, the media commands and what the source buffers hold,
// and `probe:` lines at the end. Nothing of the page is kept in the repository: pages and scripts are
// cached under --cache (default /tmp/yt-probe).
'use strict';
const vm = require('vm');
const fs = require('fs');
const path = require('path');
const cp = require('child_process');
const nodeCrypto = require('crypto');

const argv = process.argv.slice(2);
const opt = (name, dflt) => { const i = argv.indexOf(name); if (i < 0) return dflt; const v = argv[i + 1]; argv.splice(i, 2); return v; };
const flag = (name) => { const i = argv.indexOf(name); if (i < 0) return false; argv.splice(i, 1); return true; };
const SECS = +opt('--secs', '40');
const CACHE = opt('--cache', '/tmp/yt-probe');
const DIR = opt('--dir', path.join(__dirname, '..', 'user-apps/jsrun/src/bin/jsrun'));
const ENGINE = opt('--engine', 'qjs');
const VERBOSE = flag('-v');
const PLAY = flag('--play');
const NETLOG = flag('--net-log');
const W = +opt('--w', '960');
const H = +opt('--h', '540');
const REFERER = opt('--referer', '');
const PAGE = argv[0] || 'https://www.youtube.com/embed/aqz-KE-bpKQ';
const UA = 'Mozilla/5.0 (X11; Kitsune x86_64) Kitsune/0.1.0';
fs.mkdirSync(CACHE, { recursive: true });

const rs = fs.readFileSync(path.join(DIR, '..', 'jsrun.rs'), 'utf8');
const block = rs.slice(rs.indexOf('const DOM_JS'));
const files = [...block.slice(0, block.indexOf(';')).matchAll(/include_str!\("jsrun\/([\w.]+\.js)"\)/g)].map((m) => m[1]);
const prelude = files.map((f) => fs.readFileSync(path.join(DIR, f), 'utf8')).join('\n');

const t0 = Date.now();
const stamp = () => ((Date.now() - t0) / 1000).toFixed(2).padStart(6);
const out = (s) => console.log(stamp() + ' ' + s);
const summary = { requests: [], errors: [], scripts: [], media: [], appended: {}, consoleLines: 0, docs: 0 };
const turn = () => new Promise((r) => setImmediate(r));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---- cookies (a small jar: enough for the probe, not the browser's rules) ----

const jar = new Map(); // domain|path|name -> {name, value, domain, hostOnly, path, httpOnly, secure}
function setCookie(line, url) {
  const u = new URL(url);
  const parts = line.split(';').map((s) => s.trim());
  const eq = parts[0].indexOf('=');
  if (eq < 1) return;
  const c = { name: parts[0].slice(0, eq).trim(), value: parts[0].slice(eq + 1).trim(), domain: u.hostname, hostOnly: true, path: '/', httpOnly: false, secure: false };
  let expired = false;
  for (const p of parts.slice(1)) {
    const [k, ...v] = p.split('=');
    const key = k.trim().toLowerCase();
    const val = v.join('=').trim();
    if (key === 'domain' && val) { c.domain = val.replace(/^\./, '').toLowerCase(); c.hostOnly = false; }
    else if (key === 'path' && val.startsWith('/')) c.path = val;
    else if (key === 'httponly') c.httpOnly = true;
    else if (key === 'secure') c.secure = true;
    else if (key === 'max-age' && +val <= 0) expired = true;
    else if (key === 'expires' && Date.parse(val) < Date.now()) expired = true;
  }
  if (!c.hostOnly && !(u.hostname === c.domain || u.hostname.endsWith('.' + c.domain))) return;
  const key = c.domain + '|' + c.path + '|' + c.name;
  if (expired) jar.delete(key); else jar.set(key, c);
}
function cookiesFor(url, forScript) {
  const u = new URL(url);
  const list = [];
  for (const c of jar.values()) {
    const dom = c.hostOnly ? u.hostname === c.domain : u.hostname === c.domain || u.hostname.endsWith('.' + c.domain);
    if (!dom || !u.pathname.startsWith(c.path) || (c.secure && u.protocol !== 'https:') || (forScript && c.httpOnly)) continue;
    list.push(c.name + '=' + c.value);
  }
  return list.join('; ');
}
const site = (host) => host.split('.').slice(-2).join('.');

// ---- HTTP through curl ----

function curl(method, url, headers, body, follow) {
  return new Promise((resolve) => {
    const tag = nodeCrypto.randomBytes(6).toString('hex');
    const hf = path.join(CACHE, 'h-' + tag), bf = path.join(CACHE, 'b-' + tag), qf = path.join(CACHE, 'q-' + tag);
    const a = ['-sS', '-X', method, '-D', hf, '-o', bf, '-w', '%{http_code} %{url_effective}', '--max-time', '60', '-A', UA, '-H', 'Accept-Encoding: identity'];
    if (follow) a.push('-L', '--max-redirs', '10');
    for (const [k, v] of headers) a.push('-H', k + ': ' + v);
    if (body && body.length) { fs.writeFileSync(qf, body); a.push('--data-binary', '@' + qf); }
    if (method === 'HEAD') a.splice(a.indexOf('-X'), 2, '-I');
    a.push(url);
    const started = Date.now();
    cp.execFile('curl', a, { encoding: 'utf8', maxBuffer: 1 << 20 }, (err, stdout, stderr) => {
      const ms = Date.now() - started;
      let head = '';
      try { head = fs.readFileSync(hf, 'latin1'); } catch (e) { /* none */ }
      let data = Buffer.alloc(0);
      try { data = fs.readFileSync(bf); } catch (e) { /* none */ }
      for (const f of [hf, bf, qf]) { try { fs.unlinkSync(f); } catch (e) { /* gone */ } }
      if (err && !head) { resolve({ error: (stderr || String(err)).trim(), ms }); return; }
      // The last block of headers is the final answer; Set-Cookie of every hop goes to the jar.
      const blocks = head.split(/\r?\n\r?\n/).filter((b) => /^HTTP\//.test(b));
      let hop = url;
      for (const b of blocks) {
        for (const l of b.split(/\r?\n/).slice(1)) {
          const i = l.indexOf(':');
          if (i > 0 && l.slice(0, i).toLowerCase() === 'set-cookie') setCookie(l.slice(i + 1).trim(), hop);
          if (i > 0 && l.slice(0, i).toLowerCase() === 'location') { try { hop = new URL(l.slice(i + 1).trim(), hop).href; } catch (e) { /* bad */ } }
        }
      }
      const last = (blocks[blocks.length - 1] || '').split(/\r?\n/);
      const m = /^HTTP\/[\d.]+ (\d+) ?(.*)$/.exec(last[0] || '') || [];
      const hs = [];
      for (const l of last.slice(1)) { const i = l.indexOf(':'); if (i > 0) hs.push([l.slice(0, i).trim().toLowerCase(), l.slice(i + 1).trim()]); }
      const [code, eff] = stdout.split(' ');
      resolve({ status: +(m[1] || code || 0), statusText: m[2] || '', headers: hs, body: data, url: eff || url, redirected: blocks.length > 1, ms });
    });
  });
}

async function cached(url) {
  const f = path.join(CACHE, nodeCrypto.createHash('sha1').update(url).digest('hex').slice(0, 16));
  if (fs.existsSync(f)) return fs.readFileSync(f);
  const r = await curl('GET', url, [['Cookie', cookiesFor(url)]].filter((h) => h[1]), null, true);
  if (r.error || r.status !== 200) throw new Error('fetching ' + url + ': ' + (r.error || r.status));
  fs.writeFileSync(f, r.body);
  return r.body;
}

// ---- the page: QuickJS (the real page process built for the host, tools/jsrun-host.c) or Node's vm ----

function pageLine(kind, s) {
  summary.consoleLines++;
  if (/^Uncaught|ran for too long/.test(s)) summary.errors.push(s.split('\n').slice(0, 4).join(' | '));
  const m = /^jsrun: (.*) \((\d+) bytes\) ran in (\d+) ms$/.exec(s);
  if (m) summary.scripts.push([m[1], +m[2], +m[3]]);
  out('[page ' + kind + '] ' + s.slice(0, VERBOSE ? 4000 : 600));
}
function safeJson(x) { try { return JSON.stringify(x); } catch (e) { return String(x); } }
const sent = [];

function nodeEngine() {
  const plog = (kind) => (...a) => pageLine(kind, a.map((x) => (typeof x === 'string' ? x : x instanceof Error ? x.stack || String(x) : safeJson(x))).join(' '));
  const quiet = () => {};
  const ctx = { console: { log: plog('log'), error: plog('error'), warn: plog('warn'), info: plog('info'), debug: VERBOSE ? plog('debug') : quiet, trace: quiet, group: quiet, groupEnd: quiet, groupCollapsed: quiet, time: quiet, timeEnd: quiet, assert: quiet, table: quiet, dir: quiet }, performance: { now: () => Date.now() - t0 } };
  ctx.globalThis = ctx;
  vm.createContext(ctx);
  vm.runInContext('delete globalThis.Intl;', ctx); // QuickJS has none: intl.js provides it.
  ctx.__kjs_native = {
    send: (s) => { sent.push(Buffer.from(s, 'utf8')); return 0; },
    sendBytes: (h, ab) => { sent.push(Buffer.concat([Buffer.from(h, 'utf8'), ab ? Buffer.from(ab) : Buffer.alloc(0)])); return 0; },
    random: (n) => { const b = nodeCrypto.randomBytes(n); return b.buffer.slice(b.byteOffset, b.byteOffset + n); },
    evalScript: (code, name) => {
      const st = Date.now();
      try { vm.runInContext(code, ctx, { filename: name }); return true; } catch (e) { ctx.console.error('Uncaught ' + e + '\n' + (e && e.stack)); return false; } finally {
        const ms = Date.now() - st;
        if (ms >= 250) ctx.console.error('jsrun: ' + name.slice(0, 120) + ' (' + code.length + ' bytes) ran in ' + ms + ' ms');
      }
    },
  };
  vm.runInContext(prelude, ctx, { filename: 'prelude' });
  const kjs = ctx.__kjs;
  return {
    message: (text) => kjs.onMessage(text),
    data: (id, buf) => kjs.onData(id, buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.length)),
    async run() {
      for (let k = 0; k < 400; k++) {
        let ran = 0;
        try { ran = kjs.runOne(); } catch (e) { ctx.console.error('Uncaught ' + e + '\n' + e.stack); }
        await turn();
        if (!ran) break;
      }
      kjs.flush();
      const d = kjs.nextDelay();
      await sleep(Math.max(1, Math.min(d < 0 ? 20 : d, 20)));
    },
  };
}

function qjsEngine() {
  const bin = path.join(CACHE, 'jsrun-host');
  const J = path.join(DIR, '..', '..', '..');
  const srcs = [path.join(__dirname, 'jsrun-host.c'), path.join(J, 'src/glue.c'), path.join(J, 'src/page.c'),
    ...['quickjs', 'libregexp', 'libunicode', 'cutils', 'dtoa'].map((f) => path.join(J, 'vendor/quickjs', f + '.c'))];
  const newest = Math.max(...srcs.map((f) => fs.statSync(f).mtimeMs));
  if (!fs.existsSync(bin) || fs.statSync(bin).mtimeMs < newest) {
    out('probe: building ' + bin + ' (QuickJS for the host, ~40 s)');
    cp.execFileSync('clang', ['-O2', '-w', '-DCONFIG_VERSION="2026-06-04"', '-D_GNU_SOURCE', '-DSLOW_SCRIPT_MS=20', '-I' + path.join(J, 'vendor/quickjs'),
      '-I' + path.join(J, 'src'), ...srcs, '-lm', '-o', bin], { stdio: 'inherit' });
  }
  const proc = cp.spawn(bin, files.map((f) => path.join(DIR, f)), { stdio: ['pipe', 'pipe', 'pipe'] });
  let acc = Buffer.alloc(0);
  proc.stdout.on('data', (d) => {
    acc = Buffer.concat([acc, d]);
    while (acc.length >= 4) {
      const n = acc.readUInt32LE(0);
      if (acc.length < 4 + n) break;
      sent.push(Buffer.from(acc.subarray(4, 4 + n)));
      acc = acc.subarray(4 + n);
    }
  });
  let errAcc = '';
  proc.stderr.on('data', (d) => {
    errAcc += d.toString('utf8');
    // One write of the process is one report (an error with its stack): wait a moment for the rest.
    setTimeout(() => {
      if (!errAcc) return;
      const text = errAcc;
      errAcc = '';
      const lines = text.replace(/\n$/, '').split('\n');
      let cur = null;
      for (const l of lines) {
        if (cur !== null && /^\s+at |^\s*$/.test(l)) { cur += '\n' + l; continue; }
        if (cur !== null) pageLine('qjs', cur);
        cur = l;
      }
      if (cur !== null) pageLine('qjs', cur);
    }, 2);
  });
  proc.on('exit', (code, sig) => { out('probe: page process exited ' + code + ' ' + (sig || '')); summary.exited = code + ' ' + (sig || ''); });
  const frame = (b) => { const h = Buffer.alloc(4); h.writeUInt32LE(b.length); proc.stdin.write(Buffer.concat([h, b])); };
  return {
    message: (text) => frame(Buffer.from(text, 'utf8')),
    data: (id, buf) => { const h = Buffer.alloc(5); h[0] = 1; h.writeUInt32LE(id, 1); frame(Buffer.concat([h, buf])); },
    async run() { await sleep(5); },
    stop() { try { const st = fs.readFileSync('/proc/' + proc.pid + '/status', 'utf8'); out('probe: page process memory ' + (/VmHWM:\s*(\d+ kB)/.exec(st) || [])[1] + ' peak, ' + (/VmRSS:\s*(\d+ kB)/.exec(st) || [])[1] + ' now'); } catch (e) { /* gone */ } proc.kill('SIGKILL'); },
  };
}

const eng = ENGINE === 'node' ? nodeEngine() : qjsEngine();

// ---- the browser side ----

let pageUrl = PAGE;
let docHtml = '';
let layoutDue = false;
const netQueue = [];
let netBusy = false;
const aborted = new Set();

function takeSent() {
  for (const m of sent.splice(0)) {
    const kind = String.fromCharCode(m[0]);
    if (kind === 'F') { netQueue.push(parseF(m)); pumpNet(); continue; }
    if (kind === 'v') { mediaData(m); continue; }
    const rest = m.slice(1).toString();
    switch (kind) {
      case 'D': summary.docs++; docHtml = rest; layoutDue = true; break;
      case 'V': mediaCmd(rest); break;
      case 'X': aborted.add(+rest); break;
      case 'C': setCookie(rest, pageUrl); break;
      case 'N': out('navigate -> ' + rest); summary.navigated = rest; break;
      case 'O': out('window.open -> ' + rest); break;
      case 'H': out('history ' + rest); break;
      case 'K': out('worker ' + rest.slice(0, 200)); break;
      case 'L': case 'W': case 'Q': case 'S': case 'G': case 'A': case 'B': break;
      default: out('page message ' + kind + ' ' + rest.slice(0, 120));
    }
  }
}
function parseF(m) {
  const text = m.toString('latin1');
  const lines = text.slice(1).split('\n');
  const [id, method, url, mode, cred, redir, hasBody] = lines;
  const headers = [];
  let i = 7;
  for (; lines[i] !== '' && i < lines.length; i++) { const k = lines[i].indexOf(': '); headers.push([lines[i].slice(0, k), lines[i].slice(k + 2)]); }
  let off = 1;
  for (let j = 0; j <= i; j++) off += Buffer.byteLength(lines[j], 'latin1') + 1;
  return { id: +id, method, url, mode, cred, redir, hasBody: hasBody === '1', headers, body: hasBody === '1' ? m.slice(off) : null };
}

const SAFE_HEADERS = new Set(['accept', 'accept-language', 'content-language', 'content-type', 'range']);
function needsPreflight(req) {
  if (!['GET', 'HEAD', 'POST'].includes(req.method)) return true;
  for (const [k, v] of req.headers) {
    const n = k.toLowerCase();
    if (!SAFE_HEADERS.has(n)) return true;
    if (n === 'content-type' && !/^(application\/x-www-form-urlencoded|multipart\/form-data|text\/plain)\b/i.test(v)) return true;
  }
  return false;
}
async function pumpNet() {
  if (netBusy) return;
  netBusy = true;
  while (netQueue.length) {
    const req = netQueue.shift();
    if (aborted.has(req.id)) continue;
    await serve(req);
  }
  netBusy = false;
}
const answerHead = (id, o) => eng.message(JSON.stringify({ t: 'fetch', id, ev: 'head', cookie: cookiesFor(pageUrl, true), ...o }));
const answerEnd = (id) => eng.message(JSON.stringify({ t: 'fetch', id, ev: 'end' }));
const answerErr = (id, message) => eng.message(JSON.stringify({ t: 'fetch', id, ev: 'error', message }));
async function serve(req) {
  const page = new URL(pageUrl);
  const target = new URL(req.url);
  const cross = target.origin !== page.origin;
  const rec = { method: req.method, url: req.url, status: 0, bytes: 0, ms: 0, sent: req.body ? req.body.length : 0, mode: req.mode };
  summary.requests.push(rec);
  const hs = req.headers.filter(([k]) => !/^(cookie|host|origin|referer|content-length|sec-|proxy-)/i.test(k));
  if (cross || !['GET', 'HEAD'].includes(req.method)) hs.push(['Origin', page.origin]);
  hs.push(['Referer', cross ? page.origin + '/' : pageUrl]);
  const sendCookies = req.cred === 'include' || (req.cred !== 'omit' && !cross);
  if (sendCookies && site(target.hostname) === site(page.hostname)) { const c = cookiesFor(req.url); if (c) hs.push(['Cookie', c]); }
  if (cross && req.mode === 'cors' && needsPreflight(req)) {
    const names = req.headers.map(([k]) => k.toLowerCase()).filter((k) => !SAFE_HEADERS.has(k)).join(',');
    const pf = await curl('OPTIONS', req.url, [['Origin', page.origin], ['Access-Control-Request-Method', req.method], ...(names ? [['Access-Control-Request-Headers', names]] : [])], null, false);
    const acao = (pf.headers || []).find(([k]) => k === 'access-control-allow-origin');
    if (pf.error || pf.status < 200 || pf.status > 299 || !acao || (acao[1] !== '*' && acao[1] !== page.origin)) {
      rec.status = 'preflight ' + (pf.error || pf.status);
      out('net: CORS preflight failed ' + req.method + ' ' + req.url.slice(0, 160) + ' -> ' + (pf.error || pf.status));
      answerErr(req.id, 'CORS preflight failed');
      return;
    }
  }
  const r = await curl(req.method, req.url, hs, req.body, req.redir !== 'manual');
  rec.ms = r.ms;
  if (r.error) { rec.status = 'error'; out('net: ' + req.method + ' ' + req.url.slice(0, 160) + ' failed: ' + r.error); answerErr(req.id, r.error); return; }
  rec.status = r.status;
  rec.bytes = r.body.length;
  rec.type = (r.headers.find(([k]) => k === 'content-type') || [])[1] || '';
  // The player's API and the media requests, kept for reading (CACHE/dump-N.{url,req,res,head}).
  if (/youtubei\/v1\/player|videoplayback|googlevideo/.test(req.url)) {
    const n = summary.requests.length;
    const base = path.join(CACHE, 'dump-' + n);
    fs.writeFileSync(base + '.url', req.method + ' ' + req.url + '\n' + req.headers.map((h) => h.join(': ')).join('\n') + '\n');
    if (req.body) fs.writeFileSync(base + '.req', req.body);
    fs.writeFileSync(base + '.res', r.body);
    fs.writeFileSync(base + '.head', r.status + '\n' + r.headers.map((h) => h.join(': ')).join('\n') + '\n');
  }
  if (NETLOG || !/\.(png|jpg|gif|webp|woff2?|css)(\?|$)/.test(req.url)) out('net: ' + req.method + ' ' + req.url.slice(0, 200) + ' -> ' + r.status + ' ' + r.body.length + ' bytes ' + r.ms + ' ms' + (req.body ? ' (sent ' + req.body.length + ')' : '') + ' ' + req.mode);
  if (aborted.has(req.id)) return;
  let type = 'basic';
  let headers = r.headers.filter(([k]) => k !== 'set-cookie' && k !== 'set-cookie2');
  if (cross && req.mode === 'script') {
    // A <script> the page added: no CORS, but only a JavaScript answer is handed over.
    const ct = (r.headers.find(([k]) => k === 'content-type') || [])[1] || '';
    if (!/^(text|application)\/(x-)?(javascript|ecmascript)\b/i.test(ct)) { answerErr(req.id, 'not a script (' + ct + ')'); return; }
    headers = [];
  } else if (cross) {
    if (req.mode === 'no-cors') { answerHead(req.id, { status: 0, statusText: '', url: '', redirected: false, type: 'opaque', headers: [] }); answerEnd(req.id); return; }
    const acao = (r.headers.find(([k]) => k === 'access-control-allow-origin') || [])[1];
    const acac = (r.headers.find(([k]) => k === 'access-control-allow-credentials') || [])[1];
    const ok = acao === page.origin || (acao === '*' && req.cred !== 'include');
    if (!ok || (req.cred === 'include' && acac !== 'true')) {
      out('net: CORS blocked ' + req.url.slice(0, 160) + ' (Allow-Origin ' + acao + ', credentials ' + req.cred + '/' + acac + ')');
      rec.status = 'cors ' + r.status;
      answerErr(req.id, 'CORS: blocked');
      return;
    }
    type = 'cors';
    const expose = new Set(((r.headers.find(([k]) => k === 'access-control-expose-headers') || [])[1] || '').toLowerCase().split(/\s*,\s*/));
    const safe = new Set(['cache-control', 'content-language', 'content-length', 'content-type', 'expires', 'last-modified', 'pragma']);
    headers = headers.filter(([k]) => safe.has(k) || expose.has(k) || expose.has('*'));
  }
  answerHead(req.id, { status: r.status, statusText: r.statusText, url: r.url, redirected: r.redirected, type, headers });
  for (let i = 0; i < r.body.length; i += 256 * 1024) {
    eng.data(req.id, r.body.subarray(i, Math.min(r.body.length, i + 256 * 1024)));
    await turn();
  }
  answerEnd(req.id);
}

// ---- a fake decoder: the WebM timestamps of what is appended become what the source buffers hold ----

const players = new Map(); // uid -> {srcs: Map(src -> {bytes, ranges, total}), paused, ct, d, w, h, rs}
const S = 1e9;
function player(id) {
  if (!players.has(id)) players.set(id, { srcs: new Map(), paused: true, ct: 0, d: -2, w: 0, h: 0, rs: 0, last: Date.now() });
  return players.get(id);
}
const mstate = (id, p, ev) => eng.message(JSON.stringify({ t: 'media', id, ev, ct: Math.round(p.ct * S), p: p.paused ? 1 : 0, e: 0, sk: 0, rs: p.rs, w: p.w, h: p.h, v: 1, a: 1, d: p.d, vol: 100, mu: 0, err: 0 }));
function mediaCmd(text) {
  const [op, ids, ...rest] = text.split(' ');
  const id = +ids;
  summary.media.push(text.slice(0, 160));
  out('media: ' + text.slice(0, 200));
  const p = player(id);
  if (op === 'play') { p.paused = false; p.last = Date.now(); mstate(id, p, p.rs >= 3 ? ['play', 'playing'] : ['play', 'waiting']); }
  else if (op === 'pause') { p.paused = true; mstate(id, p, ['pause']); }
  else if (op === 'seek') { p.ct = +rest[0] / S; mstate(id, p, ['seeking']); mstate(id, p, ['timeupdate', 'seeked']); }
  else if (op === 'dur') { p.d = +rest[0] < 0 ? -1 : +rest[0]; mstate(id, p, ['durationchange']); }
  else if (op === 'rm') { const s = p.srcs.get(+rest[0]); eng.message(JSON.stringify({ t: 'media', id, buf: [+rest[0], 0, 0, s ? s.ranges : []] })); }
  else if (op === 'reset') { const s = p.srcs.get(+rest[0]); if (s && rest[1] === '-1') s.bytes = Buffer.alloc(0); }
  else if (op === 'src') out('media: the browser would fetch ' + rest.join(' ') + ' itself');
}
function mediaData(m) {
  const nl = m.indexOf(10);
  const [id, src, seq] = m.slice(1, nl).toString().split(' ').map(Number);
  const data = m.slice(nl + 1);
  const p = player(id);
  if (!p.srcs.has(src)) p.srcs.set(src, { bytes: Buffer.alloc(0), ranges: [], total: 0 });
  const s = p.srcs.get(src);
  s.bytes = Buffer.concat([s.bytes, data]);
  s.total += data.length;
  summary.appended[id + '/' + src] = s.total;
  const info = webmInfo(s.bytes);
  if (info.first !== null) s.ranges = [[Math.round(info.first * S), Math.round((info.last + 0.04) * S)]];
  if (info.w && !p.w) { p.w = info.w; p.h = info.h; }
  if (info.codec) s.codec = info.codec;
  eng.message(JSON.stringify({ t: 'media', id, buf: [src, seq, 0, s.ranges] }));
  out('media: append ' + id + '/' + src + ' ' + data.length + ' bytes (total ' + s.total + ')' + (info.codec ? ' ' + info.codec : '') + (info.w ? ' ' + info.w + 'x' + info.h : '') + ' holds ' + JSON.stringify(s.ranges.map((r) => r.map((x) => +(x / S).toFixed(2)))));
  const all = [...p.srcs.values()];
  if (p.rs < 1 && info.codec) { p.rs = 1; mstate(id, p, ['loadedmetadata']); }
  if (p.rs < 4 && all.every((x) => x.ranges.length)) { p.rs = 4; mstate(id, p, p.paused ? ['loadeddata', 'canplay', 'canplaythrough'] : ['loadeddata', 'canplay', 'canplaythrough', 'playing']); }
}
// Playing: the clock runs while every source holds data ahead (a timeupdate every 250 ms).
setInterval(() => {
  for (const [id, p] of players) {
    if (p.paused || p.rs < 3) continue;
    const ends = [...p.srcs.values()].map((s) => (s.ranges.length ? s.ranges[0][1] / S : 0));
    const end = ends.length ? Math.min(...ends) : 0;
    const now = Date.now();
    p.ct = Math.min(end, p.ct + (now - p.last) / 1000);
    p.last = now;
    mstate(id, p, ['timeupdate']);
    summary.playedTo = Math.max(summary.playedTo || 0, p.ct);
  }
}, 250);

// EBML: the codec, the size and the first and last block times of a WebM byte stream.
function webmInfo(b) {
  const res = { first: null, last: null, codec: '', w: 0, h: 0 };
  let scale = 1e6;
  const vint = (i, keep) => {
    const x = b[i];
    if (x === undefined) return null;
    let len = 1, mask = 0x80;
    while (len <= 8 && !(x & mask)) { len++; mask >>= 1; }
    if (len > 8 || i + len > b.length) return null;
    let v = keep ? x : x & (mask - 1);
    let allOnes = (x & (mask - 1)) === mask - 1;
    for (let k = 1; k < len; k++) { v = v * 256 + b[i + k]; if (b[i + k] !== 255) allOnes = false; }
    return { v, len, unknown: !keep && allOnes };
  };
  const uint = (at, n) => { let v = 0; for (let k = at; k < at + n; k++) v = v * 256 + b[k]; return v; };
  const MASTER = new Set([0x18538067, 0x1F43B675, 0x1654AE6B, 0xAE, 0xE0, 0xE1, 0xA0, 0x1549A966]);
  let cluster = 0;
  const walk = (i, end) => {
    while (i < end) {
      const id = vint(i, true);
      if (!id) return;
      const sz = vint(i + id.len, false);
      if (!sz) return;
      const at = i + id.len + sz.len;
      const stop = sz.unknown ? end : Math.min(end, at + sz.v);
      if (MASTER.has(id.v)) { walk(at, stop); i = stop; continue; }
      if (at + sz.v > b.length && id.v !== 0xA3) return;
      if (id.v === 0x2AD7B1) scale = uint(at, sz.v);
      else if (id.v === 0x86) res.codec = b.slice(at, at + sz.v).toString();
      else if (id.v === 0xB0) res.w = uint(at, sz.v);
      else if (id.v === 0xBA) res.h = uint(at, sz.v);
      else if (id.v === 0xE7) cluster = uint(at, sz.v);
      else if (id.v === 0xA3 || id.v === 0xA1) {
        const tn = vint(at, false);
        if (tn && at + tn.len + 2 <= b.length) {
          let rel = (b[at + tn.len] << 8) | b[at + tn.len + 1];
          if (rel & 0x8000) rel -= 0x10000;
          const t = ((cluster + rel) * scale) / 1e9;
          if (res.first === null || t < res.first) res.first = t;
          if (res.last === null || t > res.last) res.last = t;
        }
      }
      i = stop;
    }
  };
  walk(0, b.length);
  return res;
}

// ---- layout: every element of the last document gets a box (the viewport's size) ----

function sendLayout() {
  layoutDue = false;
  const recs = [];
  for (const m of docHtml.matchAll(/<([a-z][\w-]*)\s+data-kjs="(\d+)"([^>]*)>/g)) {
    const hidden = /\shidden[\s=>]|display:\s*none/.test(m[3]) || /^(head|title|meta|link|style|script)$/.test(m[1]);
    recs.push([+m[2], hidden]);
  }
  const words = new Int32Array(9 + recs.length * 13);
  words.set([1, W, H, 0, 0, W, H, 100, recs.length]);
  recs.forEach(([uid, hidden], i) => {
    words.set([uid, 0, 0, hidden ? 0 : W, hidden ? 0 : H, 0, 0, (16 << 16) | ((hidden ? 0 : 1) << 11) | 256, 0, 0, 0, 0, 0], 9 + i * 13);
  });
  eng.data(0, Buffer.from(words.buffer));
}
const uidOf = (re) => { for (const m of docHtml.matchAll(/<([a-z][\w-]*)\s+data-kjs="(\d+)"([^>]*)>/g)) if (re.test(m[0])) return +m[2]; return 0; };

// ---- the init message from the page, as kitsune_core::web::script sends it ----

async function buildInit(html) {
  const parser = { console };
  parser.globalThis = parser;
  vm.createContext(parser);
  parser.__kjs_native = { send() {}, evalScript() {}, sendBytes() {}, random() {} };
  vm.runInContext(fs.readFileSync(path.join(DIR, 'dom.js'), 'utf8'), parser);
  const nodes = parser.__kjs.dom.parseHTML(html);
  let uid = 1;
  let elems = 0;
  const classic = (t) => t === null || /^(|text\/javascript|application\/javascript|application\/x-javascript|text\/ecmascript|application\/ecmascript|text\/jscript)$/.test(t.trim().toLowerCase().split(';')[0].trim());
  const list = [];
  const conv = (kids) => {
    const res = [];
    for (const n of kids) {
      if (n.nodeType === 3) { res.push(n._data); continue; }
      if (n.nodeType !== 1 || n.localName === 'style') continue;
      const attrs = {};
      for (const [k, v] of n._attrs) attrs[k] = v;
      if (n.localName === 'script') {
        // In the tree, empty (as the browser's parser keeps it); its text goes in `scripts`.
        if (classic(n.getAttribute('type')) && (n.getAttribute('src') || n.textContent.trim())) list.push({ src: n.getAttribute('src'), text: n.textContent, el: elems });
        elems++;
        res.push(['script', uid++, attrs, []]);
        continue;
      }
      res.push([n.localName, uid++, attrs, conv(n.childNodes)]);
    }
    return res;
  };
  const dom = conv(nodes).filter((n) => typeof n !== 'string' || n.trim());
  const scripts = [];
  let inline = 0;
  for (const s of list) {
    if (s.src) {
      const u = new URL(s.src, pageUrl).href;
      try { const code = await cached(u); scripts.push([u, code, s.el]); out('script fetched ' + u.slice(0, 140) + ' (' + code.length + ' bytes)'); } catch (e) { out('script failed ' + u + ': ' + e.message); }
    } else scripts.push(['inline-' + ++inline, Buffer.from(s.text, 'utf8'), s.el]);
  }
  return { dom, scripts };
}

(async () => {
  out('probe: page ' + PAGE + ' (' + files.length + ' prelude files, engine ' + ENGINE + ')');
  const r = await curl('GET', PAGE, REFERER ? [['Referer', REFERER]] : [], null, true);
  if (r.error) { out('probe: page failed ' + r.error); process.exit(2); }
  pageUrl = r.url;
  const html = r.body.toString('utf8');
  out('probe: page ' + r.status + ' ' + html.length + ' bytes from ' + pageUrl + ' in ' + r.ms + ' ms; cookies: ' + [...jar.values()].map((c) => c.name).join(','));
  fs.writeFileSync(path.join(CACHE, 'page.html'), html);
  const { dom, scripts } = await buildInit(html);
  out('probe: ' + scripts.length + ' scripts, ' + scripts.reduce((n, s) => n + s[1].length, 0) + ' bytes');
  eng.message(JSON.stringify({ t: 'init', url: pageUrl, ua: UA, lang: 'en', ref: REFERER, cookie: cookiesFor(pageUrl, true), w: W, h: H, quirks: false, dom, wait: scripts.length }));
  for (const [name, code, el] of scripts) eng.message(Buffer.concat([Buffer.from(JSON.stringify({ t: 'script', name, el }) + '\n'), code]).toString('utf8'));
  eng.message(JSON.stringify({ t: 'view', w: W, h: H, sx: 0, sy: 0, pw: W, ph: H, z: 100 }));
  eng.message(JSON.stringify({ t: 'store', items: {} }));
  eng.message(JSON.stringify({ t: 'hist', i: 0, len: 1 }));
  let clicked = false;
  const end = Date.now() + SECS * 1000;
  while (Date.now() < end && !summary.exited) {
    await eng.run();
    takeSent();
    if (layoutDue) sendLayout();
    // --play: a click in the middle of the player once it is there (what a person does on an embed).
    if (PLAY && !clicked && Date.now() - t0 > 8000) {
      const uid = uidOf(/ytp-large-play-button|ytp-cued-thumbnail-overlay|html5-video-player/);
      if (uid) { clicked = true; out('probe: click on element ' + uid); eng.message(JSON.stringify({ t: 'click', uid, x: W / 2, y: H / 2, cx: W / 2, cy: H / 2, seq: 0, n: 1 })); }
    }
  }
  out('probe: ---- summary ----');
  out('probe: documents sent ' + summary.docs + ', console lines ' + summary.consoleLines + ', uncaught ' + summary.errors.length + (summary.exited ? ', process exited ' + summary.exited : ''));
  for (const [n, len, ms] of summary.scripts) out('probe: slow script ' + n.slice(0, 100) + ' ' + len + ' bytes ' + ms + ' ms');
  for (const e of [...new Set(summary.errors)].slice(0, 25)) out('probe: error ' + e.slice(0, 500));
  for (const q of summary.requests) out('probe: request ' + q.method + ' ' + q.url.slice(0, 150) + ' -> ' + q.status + ' ' + q.bytes + ' bytes');
  const video = /<video[^>]*>/.exec(docHtml);
  out('probe: video element ' + (video ? video[0].slice(0, 300) : 'no'));
  out('probe: appended ' + JSON.stringify(summary.appended) + ', played to ' + (summary.playedTo || 0).toFixed(2) + ' s');
  const player = /<div[^>]*class="[^"]*html5-video-player[^"]*"/.exec(docHtml);
  out('probe: player element ' + (player ? player[0].slice(0, 300) : 'none'));
  const err = /class="ytp-error-content-wrap-reason"[^>]*>([\s\S]{0,400}?)<\/div>/.exec(docHtml);
  if (err) out('probe: player error text: ' + err[1].replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim());
  fs.writeFileSync(path.join(CACHE, 'final.html'), docHtml);
  if (eng.stop) eng.stop();
  process.exit(0);
})().catch((e) => { out('probe: harness error ' + (e.stack || e)); process.exit(2); });
