// Kitsune jsrun --page: the rest of the window.
//
// `window.open` (a new tab: `O` + address), `postMessage` to the same window, `MessageChannel`,
// `BroadcastChannel` (this page only), the URL parts of `<a>` (`a.pathname`...) and absolute `href`/
// `src`, more of `navigator` (`clipboard`, `permissions`, `serviceWorker`, `storage`...), and the APIs
// a page may test for that are refused without breaking it: `Notification` (denied), service workers
// (rejected), `WebSocket` (fails like an unreachable server). See docs/design/tls-browser.md §14.
(function (global, native, kjs) {
  'use strict';
  const D = kjs.dom;
  const { dispatch, Element } = D;
  const send = native.send;
  const define = (name, value) => {
    Object.defineProperty(global, name, { value, writable: true, configurable: true, enumerable: false });
  };
  const later = (f) => global.setTimeout(f, 0);
  const P = Element.prototype;

  // ---- addresses of elements ----

  const URL_ATTR = { a: 'href', area: 'href', link: 'href', base: 'href', img: 'src', script: 'src', iframe: 'src',
    source: 'src', embed: 'src', track: 'src', input: 'src', form: 'action' };
  for (const [prop, attr] of [['href', 'href'], ['src', 'src'], ['action', 'action']]) {
    Object.defineProperty(P, prop, {
      configurable: true,
      get() {
        const v = this.getAttribute(attr);
        if (URL_ATTR[this.localName] !== attr) return v ?? '';
        if (v === null) return attr === 'action' ? location.href : '';
        try { return D.resolveURL(v, location.href); } catch (e) { return v; }
      },
      set(v) { this.setAttribute(attr, v); },
    });
  }
  for (const part of ['protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash', 'origin', 'username', 'password']) {
    Object.defineProperty(P, part, {
      configurable: true,
      get() {
        if (this.localName !== 'a' && this.localName !== 'area') return undefined;
        const href = this.href;
        const p = href && D.splitURL(href);
        if (!p) return '';
        if (part === 'origin') return p.protocol + '//' + p.host;
        if (part === 'username' || part === 'password') return '';
        return p[part];
      },
      set(v) {
        if (this.localName !== 'a' && this.localName !== 'area') return;
        const u = new URL(this.href || location.href);
        u[part] = v;
        this.setAttribute('href', u.href);
      },
    });
  }

  // ---- windows and messages ----

  define('open', function (url, target) {
    const abs = url === undefined || url === '' ? 'about:blank' : D.resolveURL(String(url), location.href);
    if (target === '_self' || target === '_top' || target === '_parent') { location.assign(abs); return global; }
    if (abs !== 'about:blank') send('O' + abs);
    return { closed: false, close() { this.closed = true; }, focus() {}, blur() {}, postMessage() {}, location: { href: abs }, document: null };
  });
  define('postMessage', function (data, origin, transfer) {
    const opts = origin && typeof origin === 'object' ? origin : { targetOrigin: origin };
    const to = opts.targetOrigin === undefined ? '/' : String(opts.targetOrigin);
    if (to !== '*' && to !== '/' && to !== location.origin) return;
    const copy = global.structuredClone ? global.structuredClone(data) : data;
    void transfer;
    later(() => dispatch(global, new global.MessageEvent('message', { data: copy, origin: location.origin, source: global })));
  });

  class MessagePort extends global.EventTarget {
    constructor() { super(); this._other = null; this._queue = []; this._started = false; this._onmessage = null; this._closed = false; }
    get onmessage() { return this._onmessage; }
    set onmessage(f) { this._onmessage = f; this.start(); }
    postMessage(data) {
      const other = this._other;
      if (!other || this._closed) return;
      const copy = global.structuredClone ? global.structuredClone(data) : data;
      other._queue.push(copy);
      if (other._started) other._flush();
    }
    start() { if (!this._started) { this._started = true; this._flush(); } }
    _flush() {
      later(() => {
        while (this._queue.length && !this._closed) {
          const ev = new global.MessageEvent('message', { data: this._queue.shift(), target: this });
          dispatch(this, ev);
          if (typeof this._onmessage === 'function') { try { this._onmessage.call(this, ev); } catch (e) { D.reportError(e); } }
        }
      });
    }
    close() { this._closed = true; }
  }
  class MessageChannel {
    constructor() { this.port1 = new MessagePort(); this.port2 = new MessagePort(); this.port1._other = this.port2; this.port2._other = this.port1; }
  }
  const channels = new Map();
  class BroadcastChannel extends global.EventTarget {
    constructor(name) {
      super();
      this.name = String(name);
      this.onmessage = null;
      const set = channels.get(this.name) || new Set();
      set.add(this);
      channels.set(this.name, set);
    }
    postMessage(data) {
      for (const c of channels.get(this.name) || []) {
        if (c === this) continue;
        const copy = global.structuredClone ? global.structuredClone(data) : data;
        later(() => {
          const ev = new global.MessageEvent('message', { data: copy, origin: location.origin });
          dispatch(c, ev);
          if (typeof c.onmessage === 'function') { try { c.onmessage(ev); } catch (e) { D.reportError(e); } }
        });
      }
    }
    close() { const s = channels.get(this.name); if (s) s.delete(this); }
  }
  define('MessagePort', MessagePort);
  define('MessageChannel', MessageChannel);
  define('BroadcastChannel', BroadcastChannel);

  // ---- navigator ----

  const nav = global.navigator;
  const reject = (name, msg) => Promise.reject(new D.DOMException(msg, name));
  Object.assign(nav, {
    platform: 'Linux x86_64',
    deviceMemory: 0.25,
    pdfViewerEnabled: false,
    webdriver: false,
    userAgentData: { brands: [{ brand: 'Kitsune', version: '1' }], mobile: false, platform: 'Linux', getHighEntropyValues: () => Promise.resolve({ platform: 'Linux', architecture: 'x86', bitness: '64', mobile: false }) },
    clipboard: {
      writeText(t) { console.log('jsrun: clipboard.writeText refused (' + String(t).length + ' characters)'); return reject('NotAllowedError', 'the page may not write the clipboard'); },
      readText() { return reject('NotAllowedError', 'the page may not read the clipboard'); },
      write() { return reject('NotAllowedError', 'the page may not write the clipboard'); },
      read() { return reject('NotAllowedError', 'the page may not read the clipboard'); },
    },
    permissions: { query: (d) => Promise.resolve({ name: d && d.name, state: 'denied', onchange: null, addEventListener() {}, removeEventListener() {} }) },
    serviceWorker: {
      controller: null,
      ready: new Promise(() => {}),
      register: () => reject('SecurityError', 'service workers are not supported'),
      getRegistration: () => Promise.resolve(undefined),
      getRegistrations: () => Promise.resolve([]),
      addEventListener() {}, removeEventListener() {},
    },
    storage: { estimate: () => Promise.resolve({ quota: 512 * 1024, usage: 0 }), persist: () => Promise.resolve(false), persisted: () => Promise.resolve(false) },
    geolocation: {
      getCurrentPosition(ok, fail) { if (fail) later(() => fail({ code: 1, message: 'User denied Geolocation', PERMISSION_DENIED: 1 })); },
      watchPosition(ok, fail) { this.getCurrentPosition(ok, fail); return 0; },
      clearWatch() {},
    },
    vibrate: () => false,
    share: () => reject('NotAllowedError', 'sharing is not supported'),
    canShare: () => false,
    registerProtocolHandler() {},
    getGamepads: () => [],
  });
  if (!nav.languages || !nav.languages.length) nav.languages = [nav.language];
  if (String(nav.language).toLowerCase().startsWith('pt') && nav.languages.length < 2) nav.languages = [nav.language, 'pt', 'en'];

  // ---- refused, without breaking the page ----

  class Notification extends global.EventTarget {
    constructor(title, opts) {
      super();
      this.title = String(title); Object.assign(this, opts || {});
      this.onerror = null; this.onshow = null; this.onclick = null; this.onclose = null;
      later(() => { const ev = new Event('error'); dispatch(this, ev); if (typeof this.onerror === 'function') this.onerror(ev); });
    }
    close() {}
    static requestPermission(cb) { if (typeof cb === 'function') later(() => cb('denied')); return Promise.resolve('denied'); }
  }
  Notification.permission = 'denied';
  Notification.maxActions = 0;
  define('Notification', Notification);

  class WebSocket extends global.EventTarget {
    constructor(url, protocols) {
      super();
      const abs = String(url).replace(/^http/, 'ws');
      if (!/^wss?:\/\//i.test(abs)) throw new D.DOMException("The URL '" + url + "' is invalid.", 'SyntaxError');
      this.url = abs; this.protocol = ''; this.extensions = ''; this.binaryType = 'blob'; this.bufferedAmount = 0;
      this.readyState = 0;
      this.onopen = this.onerror = this.onclose = this.onmessage = null;
      void protocols;
      console.log('jsrun: WebSocket ' + abs + ' is not supported yet');
      later(() => {
        this.readyState = 3;
        const err = new Event('error');
        dispatch(this, err);
        if (typeof this.onerror === 'function') this.onerror(err);
        const close = new global.CloseEvent('close', { code: 1006, reason: '', wasClean: false });
        dispatch(this, close);
        if (typeof this.onclose === 'function') this.onclose(close);
      });
    }
    send() { if (this.readyState === 0) throw new D.DOMException('Still in CONNECTING state.', 'InvalidStateError'); }
    close() { this.readyState = 3; }
  }
  Object.assign(WebSocket, { CONNECTING: 0, OPEN: 1, CLOSING: 2, CLOSED: 3 });
  class CloseEvent extends Event {
    constructor(type, init) { super(type, init); init = init || {}; this.code = init.code || 0; this.reason = init.reason || ''; this.wasClean = !!init.wasClean; }
  }
  define('CloseEvent', CloseEvent);
  if (typeof global.WebSocket !== 'function') define('WebSocket', WebSocket);

  define('frameElement', null);
  Object.defineProperty(global, 'isSecureContext', { configurable: true, get: () => /^https:|^http:\/\/(localhost|127\.)/.test(location.href) });
  Object.defineProperty(global, 'origin', { configurable: true, get: () => location.origin });
  define('crossOriginIsolated', false);
  define('reportError', (e) => D.reportError(e));
  define('external', { AddSearchProvider() {}, IsSearchProviderInstalled() {} });
  // `beforeunload` is never fired: the browser does not ask before leaving a page.
})(globalThis, globalThis.__kjs_native, globalThis.__kjs);
