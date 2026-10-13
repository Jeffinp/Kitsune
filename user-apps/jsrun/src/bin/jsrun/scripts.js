// Kitsune jsrun --page: `<script>` elements that scripts add.
//
// The document's own scripts come from the browser (dom.js runs them in order, with
// `document.currentScript`). This file runs the ones a script inserts, as browsers do ("prepare the
// script" when the element gets connected, its `src` is set, or its text arrives): an inline one runs
// at once, inside the call that inserted it; one with `src` is fetched by the browser (mode `script`:
// no CORS, a JavaScript answer only, net.js loadScript) and runs as its own task when it arrives,
// followed by `load` (or `error`) on the element. Parser-made scripts (`innerHTML`, the document's own)
// never run here (dom.js marks them started). Module scripts are not supported (`error`).
(function (global, native, kjs) {
  'use strict';
  const D = kjs.dom;
  const { document, Event } = D;
  const evalScript = native.evalScript;
  const CLASSIC = /^(|text\/javascript|application\/javascript|application\/x-javascript|text\/ecmascript|application\/ecmascript|text\/jscript|text\/livescript|application\/x-ecmascript|text\/x-javascript|text\/x-ecmascript)$/;

  const fire = (el, type) => D.tasks.push({ name: 'script ' + type, run() { D.dispatch(el, new Event(type)); } });
  function run(el, code, name) {
    const prev = document.currentScript;
    document.currentScript = el;
    try { return evalScript(code, name, true); } finally { document.currentScript = prev; }
  }

  function prepare(el) {
    if (el._kjsStarted || !el.isConnected || el.ownerDocument !== document) return;
    const src = el.getAttribute('src');
    const text = el.textContent;
    if (src === null && !text) return; // waits for its text or a src
    el._kjsStarted = true;
    const type = (el.getAttribute('type') ?? (el.hasAttribute('language') ? 'text/' + el.getAttribute('language') : '')).trim().toLowerCase();
    if (type === 'module') { console.warn('jsrun: module scripts are not supported (' + (src || 'inline') + ')'); if (src !== null) fire(el, 'error'); return; }
    if (!CLASSIC.test(type.split(';')[0].trim())) return; // JSON, templates...: data, not code
    if (src === null) { run(el, text, 'inline script'); return; }
    let url;
    try { url = new URL(src, document.baseURI || global.location.href).href; } catch (e) { url = ''; }
    if (!/^https?:/i.test(url)) { fire(el, 'error'); return; }
    const cors = el.hasAttribute('crossorigin') ? (el.getAttribute('crossorigin') === 'use-credentials' ? 'use-credentials' : 'anonymous') : '';
    kjs.net.loadScript(url, cors, (code) => {
      D.tasks.push({ name: 'script ' + url, run() {
        if (code === null) { D.dispatch(el, new Event('error')); return; }
        run(el, code, url);
        D.dispatch(el, new Event('load'));
      } });
    });
  }
  function each(n, f) {
    if (n.nodeType !== 1 && n.nodeType !== 11) return;
    if (n.localName === 'script') f(n);
    for (const k of n.childNodes.slice()) each(k, f);
  }
  D.mutators.push(function (type, target, a) {
    if (type === 'childList') {
      if (target.localName === 'script') prepare(target);
      for (const n of a) each(n, prepare);
    } else if (type === 'attributes' && a === 'src' && target.localName === 'script') {
      prepare(target);
    }
  });

  // HTMLScriptElement's own properties (src is a URL property of webapis.js).
  const P = global.HTMLScriptElement.prototype;
  const reflect = (prop, attr) => Object.defineProperty(P, prop, { configurable: true,
    get() { return this.getAttribute(attr) || ''; }, set(v) { this.setAttribute(attr, String(v)); } });
  for (const [p, a] of [['type', 'type'], ['charset', 'charset'], ['integrity', 'integrity'], ['referrerPolicy', 'referrerpolicy'], ['event', 'event'], ['htmlFor', 'for']]) reflect(p, a);
  const flag = (prop, attr) => Object.defineProperty(P, prop, { configurable: true,
    get() { return this.hasAttribute(attr); }, set(v) { if (v) this.setAttribute(attr, ''); else this.removeAttribute(attr); } });
  flag('defer', 'defer');
  flag('noModule', 'nomodule');
  // A script made by createElement is async unless told otherwise.
  Object.defineProperty(P, 'async', { configurable: true,
    get() { return this.hasAttribute('async') || (!this._kjsStarted && this._kjsAsync !== false); },
    set(v) { this._kjsAsync = !!v; if (v) this.setAttribute('async', ''); else this.removeAttribute('async'); } });
  Object.defineProperty(P, 'text', { configurable: true, get() { return this.textContent; }, set(v) { this.textContent = v; } });
  Object.defineProperty(P, 'crossOrigin', { configurable: true,
    get() { return this.getAttribute('crossorigin'); }, set(v) { if (v === null) this.removeAttribute('crossorigin'); else this.setAttribute('crossorigin', String(v)); } });
  global.HTMLScriptElement.supports = (t) => t === 'classic';
})(globalThis, globalThis.__kjs_native, globalThis.__kjs);
