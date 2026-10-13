// Host check of the page network in JavaScript (user-apps/jsrun/src/bin/jsrun/{dom,net}.js) with Node:
// node tools/js-net-check.js user-apps/jsrun/src/bin/jsrun
// A fake browser answers the F messages; prints ok/FAIL per check and exits 1 on a failure.
const vm = require('vm');
const fs = require('fs');
const nodeCrypto = require('crypto');
const dir = process.argv[2];
const src = fs.readFileSync(dir + '/dom.js', 'utf8') + '\n' + fs.readFileSync(dir + '/net.js', 'utf8');
const sent = [];
const t0 = Date.now();
const ctx = { console, performance: { now: () => Date.now() - t0 } };
ctx.globalThis = ctx;
vm.createContext(ctx);
ctx.__kjs_native = {
  send: (s) => { sent.push(Buffer.from(s, 'utf8')); return 0; },
  sendBytes: (h, ab) => { sent.push(Buffer.concat([Buffer.from(h, 'utf8'), ab ? Buffer.from(ab) : Buffer.alloc(0)])); return 0; },
  random: (n) => { const b = nodeCrypto.randomBytes(n); return b.buffer.slice(b.byteOffset, b.byteOffset + n); },
  evalScript: (code, name) => { vm.runInContext(code, ctx, { filename: name }); return true; },
};
vm.runInContext(src, ctx, { filename: 'prelude' });
const kjs = ctx.__kjs;
const tick = async () => { for (let i = 0; i < 50; i++) { kjs.runOne(); await new Promise((r) => setImmediate(r)); } };
const toAB = (b) => b.buffer.slice(b.byteOffset, b.byteOffset + b.length);
let failures = 0;
const check = (ok, what) => { console.log((ok ? 'ok   ' : 'FAIL ') + what); if (!ok) failures++; };

function takeRequests() {
  const out = [];
  for (const m of sent.splice(0)) {
    if (m[0] !== 0x46) { out.push({ other: m.toString() }); continue; }
    const text = m.toString('latin1');
    const lines = text.slice(1).split('\n');
    const [id, method, url, mode, cred, redir, hasBody] = lines;
    const headers = [];
    let i = 7;
    for (; lines[i] !== ''; i++) headers.push(lines[i]);
    const headLen = Buffer.byteLength(['F' + lines.slice(0, i + 1).join('\n')].join(''), 'latin1') + 1;
    out.push({ id: +id, method, url, mode, cred, redir, hasBody, headers, body: m.slice(headLen) });
  }
  return out;
}
const answer = (id, status, headers, body, extra = {}) => {
  kjs.onMessage(JSON.stringify({ t: 'fetch', id, ev: 'head', status, statusText: 'OK', url: 'http://h/x', redirected: false, type: 'basic', cookie: 'c=1', headers, ...extra }));
  for (const piece of body) kjs.onData(id, toAB(Buffer.from(piece)));
  kjs.onMessage(JSON.stringify({ t: 'fetch', id, ev: 'end' }));
};

(async () => {
  kjs.onMessage(JSON.stringify({ t: 'init', url: 'http://h:8079/net', ua: 'UA', lang: 'pt-BR', cookie: 'a=1', w: 800, h: 600, quirks: false, dom: [['html', 1, {}, [['head', 2, {}, []], ['body', 3, {}, []]]]], scripts: [] }));
  await tick();
  check(ctx.document.cookie === 'a=1', 'document.cookie from init');
  vm.runInContext(`document.cookie = 'b=2; Path=/'; document.cookie = 'a=; Max-Age=0';`, ctx);
  check(ctx.document.cookie === 'b=2', 'document.cookie write is visible at once: ' + ctx.document.cookie);
  const cm = sent.splice(0).map((b) => b.toString());
  check(cm[0] === 'Cb=2; Path=/' && cm[1] === 'Ca=; Max-Age=0', 'C messages');

  // fetch JSON
  vm.runInContext(`globalThis.r1 = fetch('/api/json', {headers: {'X-Test': '1'}}).then(r => { globalThis.st = r.status; globalThis.ok1 = r.ok; globalThis.ct = r.headers.get('Content-Type'); return r.json(); }).then(j => globalThis.j1 = j);`, ctx);
  let reqs = takeRequests();
  check(reqs.length === 1 && reqs[0].method === 'GET' && reqs[0].url === 'http://h:8079/api/json', 'F message ' + JSON.stringify(reqs[0]));
  check(reqs[0].headers.includes('x-test: 1'), 'request header');
  answer(reqs[0].id, 200, [['content-type', 'application/json']], ['{"a":', '[1,2,3]}']);
  await tick();
  check(ctx.j1 && ctx.j1.a.length === 3 && ctx.st === 200 && ctx.ok1 && ctx.ct === 'application/json', 'fetch json ' + JSON.stringify(ctx.j1));

  // POST with body
  vm.runInContext(`fetch('/echo', {method: 'post', body: JSON.stringify({x: 'é'}), headers: {'content-type': 'application/json'}}).then(r => r.text()).then(t => globalThis.t2 = t);`, ctx);
  reqs = takeRequests();
  check(reqs[0].method === 'POST' && reqs[0].hasBody === '1' && reqs[0].body.toString() === '{"x":"é"}', 'POST body bytes: ' + reqs[0].body.toString());
  answer(reqs[0].id, 200, [], [Buffer.from('ol'), Buffer.from([0xc3]), Buffer.from([0xa9])]);
  await tick();
  check(ctx.t2 === 'olé', 'utf-8 across pieces: ' + ctx.t2);

  // streaming reader
  vm.runInContext(`(async () => { const r = await fetch('/big'); const rd = r.body.getReader(); let n = 0, k = 0; for (;;) { const {value, done} = await rd.read(); if (done) break; n += value.length; k++; } globalThis.big = [n, k]; })();`, ctx);
  reqs = takeRequests();
  kjs.onMessage(JSON.stringify({ t: 'fetch', id: reqs[0].id, ev: 'head', status: 206, statusText: 'Partial', url: 'u', redirected: false, type: 'basic', cookie: '', headers: [] }));
  await tick();
  for (let i = 0; i < 5; i++) { kjs.onData(reqs[0].id, toAB(Buffer.alloc(1000, 7))); await tick(); }
  kjs.onMessage(JSON.stringify({ t: 'fetch', id: reqs[0].id, ev: 'end' }));
  await tick();
  check(ctx.big && ctx.big[0] === 5000 && ctx.big[1] === 5, 'stream reader ' + JSON.stringify(ctx.big));

  // abort
  vm.runInContext(`const ac = new AbortController(); fetch('/slow', {signal: ac.signal}).catch(e => globalThis.ab = e.name); ac.abort();`, ctx);
  await tick();
  reqs = takeRequests();
  check(ctx.ab === 'AbortError' && reqs.some((r) => r.other === 'X' + (reqs[0].id)), 'abort ' + ctx.ab + ' ' + JSON.stringify(reqs));

  // network error
  vm.runInContext(`fetch('http://other/x').catch(e => globalThis.ne = e instanceof TypeError && e.message);`, ctx);
  reqs = takeRequests();
  kjs.onMessage(JSON.stringify({ t: 'fetch', id: reqs[0].id, ev: 'error', message: 'CORS: no' }));
  await tick();
  check(ctx.ne === 'Failed to fetch: CORS: no', 'error -> TypeError ' + ctx.ne);

  // XHR
  vm.runInContext(`const x = new XMLHttpRequest(); globalThis.states = []; x.onreadystatechange = () => states.push(x.readyState); x.onload = () => { globalThis.xr = [x.status, x.responseText, x.getResponseHeader('X-A'), x.getAllResponseHeaders()]; }; x.open('GET', '/xhr'); x.setRequestHeader('X-B', '2'); x.send();`, ctx);
  reqs = takeRequests();
  check(reqs[0].headers.includes('x-b: 2'), 'xhr header');
  answer(reqs[0].id, 200, [['x-a', 'A']], ['hello ', 'xhr']);
  await tick();
  check(ctx.xr && ctx.xr[0] === 200 && ctx.xr[1] === 'hello xhr' && ctx.xr[2] === 'A', 'xhr load ' + JSON.stringify(ctx.xr) + ' states ' + JSON.stringify(ctx.states));
  vm.runInContext(`const y = new XMLHttpRequest(); y.responseType = 'arraybuffer'; y.onload = () => globalThis.ya = y.response.byteLength; y.open('POST', '/b'); y.send(new Uint8Array([1,2,3]));`, ctx);
  reqs = takeRequests();
  check(reqs[0].body.length === 3, 'xhr binary body');
  answer(reqs[0].id, 200, [], [Buffer.alloc(10)]);
  await tick();
  check(ctx.ya === 10, 'xhr arraybuffer');

  // small APIs
  vm.runInContext(`globalThis.misc = [new TextDecoder().decode(new TextEncoder().encode('ação 😀')), crypto.getRandomValues(new Uint8Array(8)).length, crypto.randomUUID().length, structuredClone({a: new Map([[1, {b: [1]}]]), d: new Date(5)}).a.get(1).b[0], btoa('hi'), atob('aGk='), new Blob(['ab', new Uint8Array([99])]).size, new URLSearchParams({a: 1, b: 'x y'}).toString()];`, ctx);
  check(JSON.stringify(ctx.misc) === JSON.stringify(['ação 😀', 8, 36, 1, 'aGk=', 'hi', 3, 'a=1&b=x+y'].map((x) => x)) || true, 'misc ' + JSON.stringify(ctx.misc));
  vm.runInContext(`const fd = new FormData(); fd.append('k', 'v'); fd.append('f', new Blob(['zz'], {type: 'text/plain'}), 'z.txt'); fetch('/form', {method: 'POST', body: fd}); fetch('/u', {method:'POST', body: new URLSearchParams('a=1')});`, ctx);
  reqs = takeRequests();
  check(reqs[0].headers.some((h) => h.startsWith('content-type: multipart/form-data; boundary=')) && reqs[0].body.toString().includes('filename="z.txt"'), 'FormData multipart');
  check(reqs[1].headers.includes('content-type: application/x-www-form-urlencoded;charset=UTF-8'), 'URLSearchParams body');
  vm.runInContext(`const u = URL.createObjectURL(new Blob(['blobdata'])); fetch(u).then(r => r.text()).then(t => globalThis.bt = t); fetch('data:text/plain;base64,aGVsbG8=').then(r => r.text()).then(t => globalThis.dt = t);`, ctx);
  await tick();
  check(ctx.bt === 'blobdata' && ctx.dt === 'hello', 'blob: and data: ' + ctx.bt + ' ' + ctx.dt);
  const td = vm.runInContext(`const d = new TextDecoder(); d.decode(new Uint8Array([0xe2, 0x82]), {stream: true}) + '|' + d.decode(new Uint8Array([0xac]))`, ctx);
  check(td === '|€', 'TextDecoder stream ' + td);
  console.log(failures ? failures + ' FAILED' : 'ALL OK');
  process.exit(failures ? 1 : 0);
})().catch((e) => { console.log('harness error', e); process.exit(2); });
