// Kitsune jsrun --page: the session history and the address, for single-page apps.
//
// `history.pushState`/`replaceState` change the address without loading a page: the browser is told
// (`H` + `push <url>` or `replace <url>`) so its address bar and its back/forward list follow, and the
// entry belongs to this document. `back`, `forward` and `go(n)` ask the browser (`B<n>`); when the entry
// reached is one of this document's, the browser answers with a `pop` message (`url`, index) and the
// page gets `popstate` (and `hashchange` when only the fragment changed) instead of a new load. A
// change of `location.hash`, or a click on a link to `#fragment` of this page, is the same kind of
// entry. The other `location` changes load a page (`N` + address, dom.js).
(function (global, native, kjs) {
  'use strict';
  const D = kjs.dom;
  const { dispatch } = D;
  const send = native.send;
  const location = D.location;

  // States of this document's entries, by their index in the browser's list.
  const states = new Map();
  let index = 0;
  let length = 1;
  let scrollRestoration = 'auto';

  const noHash = (u) => String(u).replace(/#.*$/, '');
  const originOf = (u) => { const p = D.splitURL(u); return p ? p.protocol + '//' + p.host : 'null'; };
  function resolve(url) {
    const abs = D.resolveURL(url === undefined || url === null ? location.href : String(url), location.href);
    if (originOf(abs) !== originOf(location.href)) {
      throw new D.DOMException("A history state object with URL '" + abs + "' cannot be created in a document with origin '" + originOf(location.href) + "'.", 'SecurityError');
    }
    return abs;
  }
  function clone(state) {
    if (state === undefined) return null;
    try { return global.structuredClone ? global.structuredClone(state) : JSON.parse(JSON.stringify(state)); } catch (e) { throw e; }
  }

  const history = {
    get length() { return length; },
    get state() { return states.has(index) ? states.get(index) : null; },
    get scrollRestoration() { return scrollRestoration; },
    set scrollRestoration(v) { if (v === 'auto' || v === 'manual') scrollRestoration = v; },
    pushState(state, title, url) {
      const abs = resolve(url);
      const st = clone(state);
      for (const k of [...states.keys()]) if (k > index) states.delete(k);
      index++;
      length = index + 1;
      states.set(index, st);
      location._url = abs;
      send('Hpush ' + abs);
    },
    replaceState(state, title, url) {
      const abs = resolve(url);
      states.set(index, clone(state));
      location._url = abs;
      send('Hreplace ' + abs);
    },
    back() { send('B-1'); },
    forward() { send('B1'); },
    go(n) {
      n = Number(n) | 0;
      if (!n) { location.reload(); return; }
      send('B' + n);
    },
  };
  Object.defineProperty(global, 'history', { value: history, writable: false, configurable: true, enumerable: true });
  define('History', function History() {});

  // The browser went to entry `i` of this document (back, forward).
  kjs.handlers.pop = function (m) {
    const old = location.href;
    const url = String(m.url);
    index = m.i | 0;
    if (m.len) length = m.len | 0;
    location._url = url;
    const ev = new global.PopStateEvent('popstate', { state: history.state });
    dispatch(global, ev);
    if (noHash(old) === noHash(url) && old !== url) {
      dispatch(global, new global.HashChangeEvent('hashchange', { oldURL: old, newURL: url }));
    }
  };
  kjs.handlers.hist = function (m) { index = m.i | 0; length = Math.max(1, m.len | 0); };

  // A new fragment of this document: an entry of its own, `hashchange` (and `popstate`), and the
  // element with that id scrolled into view.
  function goHash(abs, replace) {
    const old = location.href;
    if (old === abs) return;
    if (replace) history.replaceState(history.state, '', abs);
    else history.pushState(null, '', abs);
    const id = decodeURIComponent((/#(.*)$/.exec(abs) || ['', ''])[1]);
    const el = id && (D.document.getElementById(id) || D.document.getElementsByName(id)[0]);
    if (el && el.scrollIntoView) el.scrollIntoView();
    else if (!id && global.scrollTo) global.scrollTo(0, 0);
    dispatch(global, new global.PopStateEvent('popstate', { state: history.state }));
    dispatch(global, new global.HashChangeEvent('hashchange', { oldURL: old, newURL: abs }));
  }
  kjs.goHash = goHash;
  // Load `url` unless only its fragment differs from this page's.
  function go(url, replace) {
    const abs = D.resolveURL(String(url), location.href);
    if (/#/.test(abs) && noHash(abs) === noHash(location.href)) { goHash(abs, replace); return; }
    D.navigate(abs);
  }
  const part = (name, build) => Object.defineProperty(location, name, {
    configurable: true, enumerable: true,
    get() { return location._part(name); },
    set(v) {
      const p = D.splitURL(location.href);
      if (!p) return;
      go(build(p, String(v)), false);
    },
  });
  const join = (p) => p.protocol + '//' + p.host + p.pathname + p.search + p.hash;
  Object.defineProperty(location, 'href', {
    configurable: true, enumerable: true,
    get() { return location._url; },
    set(v) { go(v, false); },
  });
  part('hash', (p, v) => join(Object.assign(p, { hash: v === '' ? '' : v.startsWith('#') ? v : '#' + v })));
  part('search', (p, v) => join(Object.assign(p, { search: v === '' ? '' : v.startsWith('?') ? v : '?' + v, hash: '' })));
  part('pathname', (p, v) => join(Object.assign(p, { pathname: v.startsWith('/') ? v : '/' + v, search: '', hash: '' })));
  part('host', (p, v) => p.protocol + '//' + v + p.pathname + p.search + p.hash);
  part('hostname', (p, v) => p.protocol + '//' + v + (p.port ? ':' + p.port : '') + p.pathname + p.search + p.hash);
  part('port', (p, v) => p.protocol + '//' + p.hostname + (v ? ':' + v : '') + p.pathname + p.search + p.hash);
  part('protocol', (p, v) => v.replace(/:?$/, ':') + '//' + p.host + p.pathname + p.search + p.hash);
  Object.assign(location, {
    assign(u) { go(u, false); },
    replace(u) {
      const abs = D.resolveURL(String(u), location.href);
      if (/#/.test(abs) && noHash(abs) === noHash(location.href)) goHash(abs, true);
      else D.navigate(abs);
    },
    reload() { D.navigate(location._url); },
    get ancestorOrigins() { return []; },
  });
  function define(name, value) {
    Object.defineProperty(global, name, { value, writable: true, configurable: true, enumerable: false });
  }
  define('Location', function Location() {});
})(globalThis, globalThis.__kjs_native, globalThis.__kjs);
