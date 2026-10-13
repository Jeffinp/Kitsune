// Host check of the page DOM in JavaScript (user-apps/jsrun/src/bin/jsrun/*.js, in the order jsrun.rs
// loads them) with Node, playing the browser's part: it builds the `init` message from an HTML page,
// sends keys, clicks, focus and layout snapshots as the browser does (kitsune_core::web::script) and
// reads what the page sends back. Then it runs the acceptance apps of tools/site-assets (Preact +
// htm TodoMVC, Vue 3) and a list of API checks.
//   node tools/js-dom-check.js user-apps/jsrun/src/bin/jsrun tools/site-assets
// Prints ok/FAIL per check and exits 1 on a failure. `Intl` is removed first (QuickJS has none).
'use strict';
const vm = require('vm');
const fs = require('fs');
const path = require('path');
const nodeCrypto = require('crypto');
const dir = process.argv[2] || 'user-apps/jsrun/src/bin/jsrun';
const assets = process.argv[3] || 'tools/site-assets';
const only = process.argv[4] || '';
const rs = fs.readFileSync(path.join(dir, '..', 'jsrun.rs'), 'utf8');
const block = rs.slice(rs.indexOf('const DOM_JS'));
const files = [...block.slice(0, block.indexOf(';')).matchAll(/include_str!\("jsrun\/([\w.]+\.js)"\)/g)].map((m) => m[1]);
const prelude = files.map((f) => fs.readFileSync(path.join(dir, f), 'utf8')).join('\n');
const wblock = rs.slice(rs.indexOf('const WORKER_JS'));
const workerFiles = [...wblock.slice(0, wblock.indexOf(';')).matchAll(/include_str!\("jsrun\/([\w.]+\.js)"\)/g)].map((m) => m[1]);
const workerPrelude = workerFiles.map((f) => fs.readFileSync(path.join(dir, f), 'utf8')).join('\n');

let failures = 0;
const check = (ok, what) => { console.log((ok ? 'ok   ' : 'FAIL ') + what); if (!ok) failures++; };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// A page in its own context, with the browser's side played here.
class Page {
  constructor(html, url, opts = {}) {
    this.sent = [];
    this.logs = [];
    this.docs = 0;
    this.subs = '';
    this.focus = 0;
    this.answers = new Map();
    this.n = 0;
    this.seq = 0;
    this.history = [];
    this.storage = [];
    this.scroll = null;
    const t0 = Date.now();
    const log = (...a) => { const s = a.map(String).join(' '); this.logs.push(s); if (opts.verbose) console.log('   [page] ' + s); };
    const ctx = { console: { log, error: log, warn: log, info: log, debug: log }, performance: { now: () => Date.now() - t0 } };
    ctx.globalThis = ctx;
    vm.createContext(ctx);
    vm.runInContext('delete globalThis.Intl;', ctx);
    ctx.__kjs_native = {
      send: (s) => { this.sent.push(Buffer.from(s, 'utf8')); return 0; },
      sendBytes: (h, ab) => { this.sent.push(Buffer.concat([Buffer.from(h, 'utf8'), ab ? Buffer.from(ab) : Buffer.alloc(0)])); return 0; },
      random: (n) => { const b = nodeCrypto.randomBytes(n); return b.buffer.slice(b.byteOffset, b.byteOffset + n); },
      evalScript: (code, name) => {
        try { vm.runInContext(code, ctx, { filename: name }); return true; } catch (e) { log('Uncaught ' + e + '\n' + e.stack); return false; }
      },
    };
    vm.runInContext(prelude, ctx, { filename: 'prelude' });
    this.ctx = ctx;
    this.kjs = ctx.__kjs;
    const { dom, scripts } = toInit(html);
    this.kjs.onMessage(JSON.stringify({ t: 'init', url, ua: 'Mozilla/5.0 (X11; Kitsune x86_64) Kitsune/test', lang: 'pt-BR', cookie: '', w: 800, h: 600, quirks: false, dom, scripts }));
    this.kjs.onMessage(JSON.stringify({ t: 'view', w: 800, h: 600, sx: 0, sy: 0, pw: 800, ph: 600, z: 100 }));
    this.kjs.onMessage(JSON.stringify({ t: 'store', items: opts.store || {} }));
    this.kjs.onMessage(JSON.stringify({ t: 'hist', i: 0, len: 1 }));
  }
  eval(code) { return vm.runInContext(code, this.ctx); }
  get document() { return this.ctx.document; }
  q(sel) { return this.ctx.document.querySelector(sel); }
  // Run tasks, promise jobs and the timers due within `ms`, then take what the page sent.
  async pump(ms = 30) {
    const end = Date.now() + ms;
    for (let i = 0; i < 2000; i++) {
      let ran = 0;
      for (let k = 0; k < 200; k++) {
        ran = this.kjs.runOne();
        await new Promise((r) => setImmediate(r));
        if (!ran) break;
      }
      this.kjs.flush();
      this.take();
      const d = this.kjs.nextDelay();
      if (Date.now() >= end) break;
      if (d === 0) continue;
      await sleep(Math.min(d < 0 ? 5 : d, 5));
    }
  }
  take() {
    for (const m of this.sent.splice(0)) {
      const kind = String.fromCharCode(m[0]);
      const rest = m.slice(1).toString();
      switch (kind) {
        case 'D': this.docs++; this.html = rest; break;
        case 'W': this.subs = rest; break;
        case 'G': this.focus = +rest; break;
        case 'A': { const [s, v] = rest.split(' '); this.answers.set(+s, v === '1'); break; }
        case 'H': this.history.push(rest); break;
        case 'B': this.history.push('go ' + rest); break;
        case 'L': this.storage.push(JSON.parse(rest)); break;
        case 'S': this.scroll = rest; break;
        case 'Q': this.ack = +rest; break;
        case 'N': this.navigated = rest; break;
        case 'O': this.opened = rest; break;
        case 'K': this.onWorkerOp(rest); break;
        default: break;
      }
    }
  }
  // The browser moves its focus to control `el` (a click or Tab).
  focusOn(el) { this.focus = el ? el.uid : 0; this.kjs.onMessage(JSON.stringify({ t: 'focus', uid: this.focus, n: ++this.n })); }
  // A key typed into the focused control: its value after the key, as the browser computes it.
  key(k, extra = {}) {
    const el = this.focus ? this.ctx.__byUid(this.focus) : null;
    const m = { t: 'key', k, c: k.length === 1 ? (/[a-z]/i.test(k) ? 'Key' + k.toUpperCase() : /\d/.test(k) ? 'Digit' + k : 'Space') : k,
      kc: k.length === 1 ? k.toUpperCase().charCodeAt(0) : k === 'Enter' ? 13 : k === 'Backspace' ? 8 : 0, m: 0, uid: this.focus, n: ++this.n };
    const textual = el && (el.localName === 'textarea' || (el.localName === 'input' && !/checkbox|radio|submit|button/.test(el.type)));
    if (textual && k.length === 1) m.v = el.value + k;
    if (textual && k === 'Backspace') m.v = el.value.slice(0, -1);
    Object.assign(m, extra);
    this.kjs.onMessage(JSON.stringify(m));
    return m;
  }
  async type(text) { for (const ch of text) { this.key(ch); await this.pump(5); } }
  // A click on `el`: the browser focuses a control first, then sends the click (with a verdict wanted
  // for links and submit buttons).
  click(el, wantVerdict) {
    if (el && /^(input|textarea|select|button)$/.test(el.localName)) this.focusOn(el);
    const seq = wantVerdict ? ++this.seq : 0;
    this.kjs.onMessage(JSON.stringify({ t: 'click', uid: el.uid, x: 10, y: 10, cx: 10, cy: 10, seq, n: ++this.n }));
    return seq;
  }
  field(el, m) { this.kjs.onMessage(JSON.stringify(Object.assign({ t: 'field', uid: el.uid, n: ++this.n }, m))); }
  // A layout snapshot: every element of the page gets a box (stacked rows of 20 px).
  layout(list) {
    const recs = list || [];
    const words = new Int32Array(9 + recs.length * 13);
    words.set([1, 800, 600, 0, 0, 800, Math.max(600, recs.length * 20), 100, recs.length]);
    recs.forEach((r, i) => words.set([r.uid, r.x, r.y, r.w, r.h, r.color || 0, r.bg || 0, ((r.font || 16) << 16) | ((r.display ?? 1) << 11) | (r.flags ?? 256), 0, 0, 0, 0, 0], 9 + i * 13));
    this.kjs.onData(0, words.buffer);
  }
  logged(re) { return this.logs.some((l) => re.test(l)); }
  lastLog(prefix) { return [...this.logs].reverse().find((l) => l.startsWith(prefix)) || ''; }
}

// The browser's `init` from an HTML page: elements numbered in document order, scripts apart (inline
// text, or the file from the assets folder).
let parser = null;
function toInit(html) {
  if (!parser) {
    parser = { console };
    parser.globalThis = parser;
    vm.createContext(parser);
    parser.__kjs_native = { send() {}, evalScript() {}, sendBytes() {}, random() {} };
    vm.runInContext(fs.readFileSync(path.join(dir, 'dom.js'), 'utf8'), parser);
  }
  const nodes = parser.__kjs.dom.parseHTML(html);
  let uid = 1;
  const scripts = [];
  const conv = (list) => {
    const out = [];
    for (const n of list) {
      if (n.nodeType === 3) { out.push(n._data); continue; }
      if (n.nodeType !== 1) continue;
      if (n.localName === 'script') {
        const src = n.getAttribute('src');
        scripts.push(src ? [src, fs.readFileSync(path.join(assets, path.basename(src)), 'utf8')] : ['inline-' + (scripts.length + 1), n.textContent]);
        continue;
      }
      if (n.localName === 'style') continue;
      const attrs = {};
      for (const [k, v] of n._attrs) attrs[k] = v;
      const id = uid++;
      out.push([n.localName, id, attrs, conv(n.childNodes)]);
    }
    return out;
  };
  let dom = conv(nodes);
  dom = dom.filter((n) => typeof n !== 'string' || n.trim());
  // KITSUNE_INIT=<script_init binary> takes the tree from the browser's own HTML parser instead
  // (cargo build -p kitsune_core --example script_init).
  if (process.env.KITSUNE_INIT) {
    const tmp = path.join(require('os').tmpdir(), 'kjs-page-' + process.pid + '.html');
    fs.writeFileSync(tmp, html);
    const out = require('child_process').execFileSync(process.env.KITSUNE_INIT, [tmp, 'http://h/']);
    fs.unlinkSync(tmp);
    dom = JSON.parse(out.toString()).dom;
  }
  return { dom, scripts };
}

async function preactTodo() {
  console.log('-- Preact + htm TodoMVC');
  const html = fs.readFileSync(path.join(assets, 'preact.html'), 'utf8');
  const p = new Page(html, 'http://h:8279/preact', { verbose: !!process.env.V });
  p.ctx.__byUid = (u) => p.kjs.dom.byUid.get(u);
  await p.pump(200);
  check(p.logged(/^todo: preact function hooks function htm function/), 'Preact, hooks and htm loaded');
  check(p.logged(/^todo: n=0 left=0 filter=all/), 'first render: ' + p.lastLog('todo: n='));
  const input = p.q('.new-todo');
  check(!!input, 'the input is in the document');
  check(p.document.activeElement === input && p.focus === input.uid, 'input.focus() in an effect: activeElement and G message (' + p.focus + ')');
  check(/<input[^>]*class="new-todo"/.test(p.html || ''), 'the document went to the browser');
  await p.type('leite');
  await p.pump(60);
  check(input.value === 'leite' && p.lastLog('todo: n=').includes('input="leite"'), 'typing reaches the app: ' + p.lastLog('todo: n='));
  check(/value="leite"/.test(p.html), 'the browser\'s copy carries the typed value');
  check(p.ack === p.n, 'Q acknowledges the inputs (' + p.ack + '/' + p.n + ')');
  p.key('Enter');
  await p.pump(30);
  check(p.logged(/^todo: enter value="leite"/), 'keydown Enter');
  check(p.lastLog('todo: n=').startsWith('todo: n=1 left=1 filter=all shown=leite dom=1 input=""'), 'item added, field cleared: ' + p.lastLog('todo: n='));
  check(input.value === '' && /class="new-todo"[^>]*value=""|value=""[^>]*class="new-todo"/.test(p.html), 'value="" reaches the browser');
  await p.type('pao');
  p.key('Enter');
  await p.pump(30);
  await p.type('cafe');
  p.key('Enter');
  await p.pump(30);
  check(p.lastLog('todo: n=').startsWith('todo: n=3 left=3 filter=all shown=leite|pao|cafe dom=3'), 'three items: ' + p.lastLog('todo: n='));
  // Tick the second one.
  const box = p.document.querySelectorAll('.todo-list li input.toggle')[1];
  p.click(box);
  await p.pump(30);
  check(box.checked && p.lastLog('todo: n=').startsWith('todo: n=3 left=2'), 'click on a check box: change event, ' + p.lastLog('todo: n='));
  check(p.document.querySelectorAll('.todo-list li')[1].className === 'completed', 'the li gets class completed');
  // Filter with the hash links.
  const active = p.q('.filters a[href="#/active"]');
  const seq = p.click(active, true);
  await p.pump(30);
  check(p.answers.get(seq) === false, 'a #fragment link is handled in the page (verdict 0)');
  check(p.lastLog('todo: n=').includes('filter=active shown=leite|cafe dom=2'), 'hashchange filters: ' + p.lastLog('todo: n='));
  check(p.history.includes('push http://h:8279/preact#/active'), 'the address follows (H push): ' + p.history.join(','));
  check(p.eval('location.hash') === '#/active' && p.eval('history.length') === 2, 'location.hash and history.length');
  p.kjs.onMessage(JSON.stringify({ t: 'pop', url: 'http://h:8279/preact', i: 0, len: 2 }));
  await p.pump(30);
  check(p.lastLog('todo: n=').includes('filter=all shown=leite|pao+|cafe dom=3'), 'back: popstate + hashchange: ' + p.lastLog('todo: n='));
  // Remove one.
  p.click(p.document.querySelectorAll('.todo-list li button.destroy')[0]);
  await p.pump(30);
  check(p.lastLog('todo: n=').startsWith('todo: n=2 left=1 filter=all shown=pao+|cafe'), 'remove: ' + p.lastLog('todo: n='));
  check(p.storage.some((s) => s[0] === 'set' && s[1] === 'todos-preact' && /cafe/.test(s[2])), 'localStorage writes go to the browser');
  check(p.document.title === 'Preact: 1 restantes', 'document.title');
  // Focus moves away.
  p.focusOn(null);
  await p.pump(20);
  check(p.lastLog('todo: n=').includes('focus=out'), 'blur from the browser: ' + p.lastLog('todo: n='));
  // A new page with the stored list.
  const p2 = new Page(html, 'http://h:8279/preact#/completed', { store: { 'todos-preact': JSON.stringify([{ id: 7, title: 'x', done: true }, { id: 8, title: 'y', done: false }]) } });
  p2.ctx.__byUid = (u) => p2.kjs.dom.byUid.get(u);
  await p2.pump(100);
  check(p2.lastLog('todo: n=').startsWith('todo: n=2 left=1 filter=completed shown=x+ dom=1'), 'stored list and the hash route on load: ' + p2.lastLog('todo: n='));
  if (failures) for (const l of p.logs.slice(-12)) console.log('   | ' + l);
}

async function vueApp() {
  console.log('-- Vue 3');
  const html = fs.readFileSync(path.join(assets, 'vue.html'), 'utf8');
  const p = new Page(html, 'http://h:8279/vue', { verbose: !!process.env.V });
  p.ctx.__byUid = (u) => p.kjs.dom.byUid.get(u);
  await p.pump(300);
  const errs = p.logs.filter((l) => /^vue: (error|warn)|Uncaught/.test(l));
  check(!errs.length, 'no Vue errors or warnings ' + errs.slice(0, 3).join(' / '));
  check(p.logged(/^vue: mounted count=0 name="" fruits=banana\|caju dom=2 show=false extra=false color=vermelho sent="" hello="Digite seu nome."/), 'mounted: ' + p.lastLog('vue: '));
  check(/<h1[^>]*>Vue 3\.\d+\.\d+<\/h1>/.test(p.html || ''), 'the template rendered (h1)');
  const inc = p.q('#inc');
  p.click(inc);
  await p.pump(30);
  p.click(inc);
  await p.pump(30);
  check(p.lastLog('vue: count').includes('count=2') && p.lastLog('vue: count').includes('countdom=2'), '@click: ' + p.lastLog('vue: count'));
  check(inc.className === 'on' && /id="inc"[^>]*class="on"|class="on"[^>]*id="inc"/.test(p.html), ':class follows');
  check(p.document.title === 'Vue: 2', 'title');
  const name = p.q('#name');
  p.click(name);
  await p.type('Ana');
  check(p.lastLog('vue: name').includes('name="Ana"') && p.lastLog('vue: name').includes('hello="Ola, Ana!"'), 'v-model + v-if: ' + p.lastLog('vue: name'));
  const fruit = p.q('#fruit');
  p.click(fruit);
  await p.type('uva');
  p.key('Enter');
  await p.pump(30);
  check(p.lastLog('vue: fruits').includes('fruits=banana|caju|uva dom=3'), '@keyup.enter + v-for: ' + p.lastLog('vue: fruits'));
  check(fruit.value === '', 'v-model cleared the field');
  const show = p.q('#show');
  p.click(show.parentNode); // the label
  await p.pump(30);
  check(show.checked && p.lastLog('vue: show').includes('show=true extra=true'), 'label click toggles the check box, v-model: ' + p.lastLog('vue: show'));
  const sel = p.q('select');
  p.click(sel);
  p.field(sel, { si: 2 });
  await p.pump(30);
  check(p.lastLog('vue: color').includes('color=azul'), 'select change: ' + p.lastLog('vue: color'));
  check(/<option[^>]*selected[^>]*>azul|<option selected="">azul/.test(p.html), 'the selected option reaches the browser');
  const msg = p.q('form input');
  p.click(msg);
  await p.type('oi');
  const seq = ++p.seq;
  p.key('Enter', { seq });
  await p.pump(30);
  check(p.answers.get(seq) === false, 'Enter in a form: submit event, @submit.prevent (verdict 0)');
  check(p.lastLog('vue: sent').includes('sent="oi"'), 'the form handler ran: ' + p.lastLog('vue: sent'));
  const btn = p.q('form button');
  await p.type('x');
  const seq2 = p.click(btn, true);
  await p.pump(30);
  check(p.answers.get(seq2) === false && p.lastLog('vue: sent').includes('sent="x"'), 'submit button: prevented, handler ran');
  if (failures) for (const l of p.logs.slice(-12)) console.log('   | ' + l);
}

// A worker's process, as the browser starts it for a page (`K new`), with the messages carried both ways.
class WorkerProc {
  constructor(page, id, url, name, code) {
    this.page = page;
    this.id = id;
    this.logs = [];
    const t0 = Date.now();
    const log = (...a) => this.logs.push(a.map(String).join(' '));
    const ctx = { console: { log, error: log, warn: log, info: log, debug: log }, performance: { now: () => Date.now() - t0 } };
    ctx.globalThis = ctx;
    vm.createContext(ctx);
    vm.runInContext('delete globalThis.Intl;', ctx);
    ctx.__kjs_native = {
      send: (m) => this.fromWorker(String(m)),
      sendBytes: () => 0,
      random: (n) => { const b = nodeCrypto.randomBytes(n); return b.buffer.slice(b.byteOffset, b.byteOffset + n); },
      evalScript: (c, n) => { try { vm.runInContext(c, ctx, { filename: n }); return true; } catch (e) { log('Uncaught ' + e); return false; } },
    };
    vm.runInContext(workerPrelude, ctx, { filename: 'worker-prelude' });
    this.ctx = ctx;
    this.kjs = ctx.__kjs;
    this.kjs.onMessage(JSON.stringify({ t: 'winit', url, name, code }));
  }
  fromWorker(m) {
    const kind = m[0], rest = m.slice(1);
    if (kind === 'M') this.page.kjs.onMessage(JSON.stringify({ t: 'worker', id: this.id, ev: 'message', data: rest }));
    else if (kind === 'E') this.page.kjs.onMessage(JSON.stringify({ t: 'worker', id: this.id, ev: 'error', message: rest }));
    else if (kind === 'Z') { this.closed = true; this.page.kjs.onMessage(JSON.stringify({ t: 'worker', id: this.id, ev: 'end' })); }
  }
  async pump() { for (let i = 0; i < 50; i++) { if (!this.kjs.runOne()) break; await new Promise((r) => setImmediate(r)); } }
}
Page.prototype.onWorkerOp = function (rest) {
  this.workers = this.workers || new Map();
  const [op, id, ...more] = rest.split('\n');
  if (op === 'new') {
    const [url, name, ...code] = more;
    this.workers.set(+id, new WorkerProc(this, +id, url, name, code.join('\n')));
  } else if (op === 'msg') {
    const w = this.workers.get(+id);
    if (w) w.kjs.onMessage(JSON.stringify({ t: 'wmsg', data: more.join('\n') }));
  } else if (op === 'end') this.workers.delete(+id);
};

async function workers() {
  console.log('-- Workers');
  const p = new Page('<!doctype html><html><head><title>w</title></head><body><p id="o"></p></body></html>', 'http://h:8279/w');
  await p.pump(30);
  p.eval(`globalThis.wlog = [];
    const code = "onmessage = (e) => { postMessage({ got: e.data.n, sq: e.data.n * e.data.n, d: e.data.d instanceof Date, u8: Array.from(e.data.u8), m: new Map([[1, 'a']]) }); }; " +
      "postMessage('ready ' + typeof document + ' ' + typeof window + ' ' + typeof self.postMessage + ' ' + (self instanceof WorkerGlobalScope) + ' ' + location.protocol + ' ' + typeof fetch + ' ' + typeof setTimeout);";
    const w = new Worker(URL.createObjectURL(new Blob([code], { type: 'text/javascript' })));
    w.onmessage = (e) => wlog.push(e.data);
    w.postMessage({ n: 7, d: new Date(0), u8: new Uint8Array([1, 2]) });
    globalThis.w1 = w;`);
  for (let i = 0; i < 10; i++) {
    await p.pump(10);
    for (const w of (p.workers || new Map()).values()) await w.pump();
  }
  const got = p.eval('wlog');
  check(got.length === 2 && got[0] === 'ready undefined undefined function true blob: function function', 'a worker runs its script without document or window: ' + JSON.stringify(got[0]));
  check(got[1] && got[1].got === 7 && got[1].sq === 49 && got[1].d === true && got[1].u8.join() === '1,2' && got[1].m instanceof p.eval('Map') && got[1].m.get(1) === 'a', 'postMessage both ways with structured clone (Date, Uint8Array, Map)');
  p.eval(`globalThis.w2 = new Worker(URL.createObjectURL(new Blob(['setTimeout(() => close(), 1)']))); globalThis.werr = 0; globalThis.w3 = new Worker('data:text/javascript,1'); w3.onerror = () => werr++;`);
  for (let i = 0; i < 10; i++) { await p.pump(10); for (const w of (p.workers || new Map()).values()) await w.pump(); }
  check(p.eval('werr') === 1 && p.workers.size === 2, 'a third worker is refused with an error event');
  check(p.workers.get(2).closed === true && p.eval('w2._state') === 'ended', 'close() in the worker ends it');
  check(p.eval(`(() => { try { new Worker('http://other.test/w.js'); return 'no'; } catch (e) { return e.name; } })()`) === 'SecurityError', 'another origin is refused');
  p.eval('w1.terminate()');
  p.take();
  check(!p.workers.has(1), 'terminate() ends the worker');
}

async function apis() {
  console.log('-- APIs');
  const p = new Page('<!doctype html><html><head><title>t</title></head><body><div id="a" style="color: red"><p id="b">x</p><span id="c">y</span></div><form id="f" action="/go"><input name="q" value="1"><input type="checkbox" name="c" checked><select name="s"><option>a</option><option selected>b</option></select><textarea name="t">tx</textarea></form><template id="t"><li class="row">r</li></template></body></html>', 'http://h:8279/api');
  p.ctx.__byUid = (u) => p.kjs.dom.byUid.get(u);
  await p.pump(50);
  const E = (c) => p.eval(c);
  // Mutation observer.
  E(`globalThis.recs = []; new MutationObserver((r) => recs.push(...r.map((x) => x.type + ':' + (x.attributeName || x.addedNodes.length + '/' + x.removedNodes.length)))).observe(document.body, { subtree: true, childList: true, attributes: true, characterData: true });
    document.getElementById('a').setAttribute('data-x', '1'); document.getElementById('b').remove(); document.getElementById('a').append('z');`);
  await p.pump(10);
  check(E('recs.join(",")') === 'attributes:data-x,childList:0/1,childList:1/0', 'MutationObserver: ' + E('recs.join(",")'));
  // Template, fragments, cloning.
  check(E(`const t = document.getElementById('t'); const c = t.content.cloneNode(true); document.body.appendChild(c); document.querySelectorAll('li.row').length + ':' + t.content.childNodes.length + ':' + (t.content instanceof DocumentFragment)`) === '1:1:true', 'template.content');
  check(E(`document.createElement('input') instanceof HTMLInputElement && !(document.createElement('div') instanceof HTMLInputElement) && document.createElement('div') instanceof HTMLElement`) === true, 'element classes');
  // Forms.
  check(E(`const f = document.getElementById('f'); [f.elements.length, f.elements.q.value, f.q ? 1 : 0, document.querySelector('select').selectedIndex, document.querySelector('select').value, document.querySelector('textarea').value, document.querySelector('[type=checkbox]').checked].join()`) === '4,1,0,1,b,tx,true', 'form controls: ' + E(`const f2 = document.getElementById('f'); [f2.elements.length, f2.elements.q.value, document.querySelector('select').selectedIndex, document.querySelector('select').value, document.querySelector('textarea').value].join()`));
  E(`document.querySelector('[name=q]').value = 'zz'; document.querySelector('select').selectedIndex = 0; document.querySelector('[type=checkbox]').checked = false;`);
  await p.pump(10);
  check(/name="q" value="zz"/.test(p.html) && !/type="checkbox" name="c" checked/.test(p.html) && /<option[^>]*selected="">a/.test(p.html), 'script-set values reach the browser: ' + (p.html.match(/<form.*?<\/form>/) || [''])[0]);
  check(E(`document.querySelector('[name=q]').getAttribute('value')`) === '1', 'value attribute stays the default');
  // Custom elements and shadow DOM.
  E(`class XHello extends HTMLElement { static get observedAttributes() { return ['who']; }
      constructor() { super(); this.attachShadow({ mode: 'open' }).innerHTML = '<b>Ola <slot>ninguem</slot></b><slot name="end"></slot>'; globalThis.ctorRan = (globalThis.ctorRan || 0) + 1; }
      connectedCallback() { globalThis.conn = (globalThis.conn || 0) + 1; }
      attributeChangedCallback(n, o, v) { globalThis.attr = n + '=' + v; } }
    customElements.define('x-hello', XHello);
    const x = document.createElement('x-hello'); x.setAttribute('who', 'ana'); x.innerHTML = 'Ana<i slot="end">!</i>'; document.body.appendChild(x);`);
  await p.pump(10);
  check(E('[ctorRan, conn, attr, customElements.get("x-hello") !== undefined].join()') === '1,1,who=ana,true', 'custom element lifecycle: ' + E('[globalThis.ctorRan, globalThis.conn, globalThis.attr].join()'));
  check(/<x-hello[^>]*><b[^>]*>Ola Ana<\/b><i[^>]*>!<\/i><\/x-hello>/.test(p.html), 'shadow tree drawn with slots: ' + (p.html.match(/<x-hello.*?<\/x-hello>/) || [''])[0]);
  check(E(`(() => { let hit = 0; document.querySelector('x-hello').addEventListener('click', () => hit++); document.querySelector('x-hello').shadowRoot.querySelector('b').click(); return hit; })()`) === 1, 'events cross the shadow root');
  // Upgrade of an element in the document before define().
  E(`document.body.insertAdjacentHTML('beforeend', '<y-late></y-late>'); customElements.define('y-late', class extends HTMLElement { connectedCallback() { globalThis.late = 'up'; } });`);
  check(E('globalThis.late') === 'up', 'upgrade on define');
  // Layout snapshot.
  const a = E(`document.getElementById('a')`);
  p.layout([{ uid: a.uid, x: 8, y: 30, w: 300, h: 40, color: 0xff0000, flags: 256 | 1 }]);
  await p.pump(10);
  check(E(`JSON.stringify(document.getElementById('a').getBoundingClientRect())`) === '{"x":8,"y":30,"width":300,"height":40,"top":30,"right":308,"bottom":70,"left":8}', 'getBoundingClientRect');
  check(E(`[document.getElementById('a').offsetWidth, document.getElementById('a').offsetHeight, getComputedStyle(document.getElementById('a')).color, getComputedStyle(document.getElementById('a')).fontWeight, getComputedStyle(document.getElementById('a')).display, innerWidth, matchMedia('(min-width: 700px)').matches, matchMedia('(max-width: 500px)').matches].join()`) === '300,40,red,700,block,800,true,false', 'offset*, getComputedStyle, matchMedia: ' + E(`[getComputedStyle(document.getElementById('a')).color].join()`));
  E(`globalThis.io = []; new IntersectionObserver((es) => es.forEach((e) => io.push(e.isIntersecting + ':' + e.intersectionRatio))).observe(document.getElementById('a'));
     globalThis.ro = []; new ResizeObserver((es) => es.forEach((e) => ro.push(e.contentRect.width))).observe(document.getElementById('a'));`);
  await p.pump(20);
  check(E('io.join()') === 'true:1' && E('ro.join()') === '300', 'IntersectionObserver and ResizeObserver: ' + E('io.join()') + ' ' + E('ro.join()'));
  check(p.subs.split(',').includes('scroll'), 'IntersectionObserver asks for scroll events (W ' + p.subs + ')');
  p.kjs.onMessage(JSON.stringify({ t: 'view', w: 800, h: 600, sx: 0, sy: 500, pw: 800, ph: 2000, z: 100 }));
  await p.pump(20);
  check(E('io.join()') === 'true:1,false:0' && E('scrollY') === 500, 'scrolled away: ' + E('io.join()'));
  E(`document.getElementById('a').scrollIntoView()`);
  p.take();
  check(p.scroll === '0 30', 'scrollIntoView sends S: ' + p.scroll);
  // Events.
  check(E(`(() => { const out = []; const d = document.getElementById('a'); d.addEventListener('x', (e) => out.push('a' + e.detail), { once: true }); const ac = new AbortController(); d.addEventListener('x', () => out.push('b'), { signal: ac.signal }); d.dispatchEvent(new CustomEvent('x', { detail: 1 })); ac.abort(); d.dispatchEvent(new CustomEvent('x', { detail: 2 })); return out.join(); })()`) === 'a1,b', 'once and signal');
  E(`document.addEventListener('mousemove', (e) => globalThis.mm = e.clientX + ',' + e.clientY); document.getElementById('a').addEventListener('mouseenter', () => globalThis.ent = 1);`);
  p.take();
  check(p.subs.split(',').includes('move'), 'mousemove asks the browser (W)');
  p.kjs.onMessage(JSON.stringify({ t: 'mouse', e: 'move', uid: a.uid, x: 15, y: 40, n: ++p.n }));
  await p.pump(10);
  check(E('globalThis.mm') === '15,40' && E('globalThis.ent') === 1, 'mousemove and mouseenter');
  // Keys on the page (no field).
  E(`document.addEventListener('keydown', (e) => globalThis.kd = e.key + e.code + e.ctrlKey)`);
  p.focusOn(null);
  p.key('k', { m: 2 });
  await p.pump(10);
  check(E('globalThis.kd') === 'kKeyKtrue', 'keydown with ctrlKey on the document');
  // Storage, history, Intl.
  E(`localStorage.a = '1'; localStorage.setItem('b', 2); sessionStorage.setItem('s', 'x');`);
  p.take();
  check(E(`[localStorage.length, localStorage.getItem('a'), localStorage.b, Object.keys(localStorage).join('')].join()`) === '2,1,2,ab' && p.storage.length === 2, 'localStorage');
  E(`history.pushState({ p: 1 }, '', '/a/b?x=1'); history.replaceState({ p: 2 }, '', 'c');`);
  p.take();
  check(E('location.pathname + location.search + JSON.stringify(history.state)') === '/a/c{"p":2}' && p.history.slice(-2).join() === 'push http://h:8279/a/b?x=1,replace http://h:8279/a/c', 'pushState/replaceState: ' + p.history.join());
  check(E(`(() => { try { history.pushState(null, '', 'http://other/'); return 'no'; } catch (e) { return e.name; } })()`) === 'SecurityError', 'pushState refuses another origin');
  check(E(`new Intl.NumberFormat('pt-BR', { style: 'currency', currency: 'BRL' }).format(1234.5)`) === 'R$ 1.234,50', 'Intl.NumberFormat pt-BR currency');
  check(E(`(1234567.891).toLocaleString('en-US')`) === '1,234,567.891', 'toLocaleString en-US');
  check(E(`new Intl.DateTimeFormat('pt-BR', { dateStyle: 'long' }).format(new Date(Date.UTC(2026, 9, 13)))`) === '13 de outubro de 2026', 'DateTimeFormat pt-BR long');
  check(E(`new Date(Date.UTC(2026, 9, 13, 15, 4)).toLocaleString('en-US')`) === '10/13/2026, 3:04:00 PM', 'Date.toLocaleString en-US: ' + E(`new Date(Date.UTC(2026, 9, 13, 15, 4)).toLocaleString('en-US')`));
  check(E(`new Intl.PluralRules('pt-BR').select(1) + new Intl.PluralRules('en').select(0) + new Intl.RelativeTimeFormat('pt-BR', { numeric: 'auto' }).format(-1, 'day')`) === 'oneotherontem', 'PluralRules, RelativeTimeFormat');
  check(E(`['é', 'e', 'f', 'a'].sort(new Intl.Collator('pt-BR').compare).join('')`) === 'aeéf', 'Collator');
  // Range, TreeWalker, Selection.
  check(E(`(() => { const r = document.createRange(); r.selectNodeContents(document.getElementById('c')); return r.toString(); })()`) === 'y', 'Range.toString');
  check(E(`(() => { const w = document.createTreeWalker(document.getElementById('f'), NodeFilter.SHOW_ELEMENT); const out = []; while (w.nextNode()) out.push(w.currentNode.localName); return out.join(); })()`) === 'input,input,select,option,option,textarea', 'TreeWalker');
  check(E(`getSelection().rangeCount`) === 0, 'getSelection');
  // postMessage and MessageChannel.
  E(`globalThis.pm = []; addEventListener('message', (e) => pm.push(e.data.v)); postMessage({ v: 1 }, '*'); const mc = new MessageChannel(); mc.port1.onmessage = (e) => pm.push(e.data); mc.port2.postMessage(2);`);
  await p.pump(20);
  check(E('pm.sort().join()') === '1,2', 'postMessage and MessageChannel');
  E(`open('/novo')`);
  p.take();
  check(p.opened === 'http://h:8279/novo', 'window.open asks for a tab');
  if (failures) for (const l of p.logs.slice(-8)) console.log('   | ' + l);
}

(async () => {
  try {
    if (!only || only === 'apis') await apis();
    if (!only || only === 'preact') await preactTodo();
    if (!only || only === 'vue') await vueApp();
    if (!only || only === 'workers') await workers();
  } catch (e) {
    console.log('FAIL exception ' + e.stack);
    failures++;
  }
  console.log(failures ? failures + ' FAILED' : 'all ok');
  process.exit(failures ? 1 : 0);
})();
