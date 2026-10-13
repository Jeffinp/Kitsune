// Kitsune jsrun --worker: the global scope of a dedicated worker.
//
// A worker is another jsrun process (started by the browser for a page's `new Worker(url)`, see
// workers.js) with the timers, the network (net.js), `Intl` and the message codec, and without a
// document or a window. The browser sends `winit` (the script, its address and name) and `wmsg` (a
// message from the page); the worker answers `M<message>` (`postMessage`), `E<text>` (the script
// failed) and `Z` (`close()`). Messages: kitsune_core::web::script::worker.
(function (global, native, kjs) {
  'use strict';
  const D = kjs.dom;
  const send = native.send;
  const define = (name, value) => {
    Object.defineProperty(global, name, { value, writable: true, configurable: true, enumerable: false });
  };

  // No document and no window in a worker.
  for (const k of Object.getOwnPropertyNames(global)) {
    if (/^(HTML\w*Element|SVG\w*Element)$/.test(k)) delete global[k];
  }
  for (const k of ['document', 'window', 'parent', 'top', 'frames', 'opener', 'frameElement', 'history',
    'localStorage', 'sessionStorage', 'screen', 'alert', 'confirm', 'prompt', 'print', 'open', 'stop',
    'focus', 'blur', 'getComputedStyle', 'matchMedia', 'scrollTo', 'scrollBy', 'scroll', 'innerWidth',
    'innerHeight', 'outerWidth', 'outerHeight', 'devicePixelRatio', 'scrollX', 'scrollY', 'pageXOffset',
    'pageYOffset', 'getSelection', 'Node', 'Element', 'Text', 'Comment', 'Document', 'DocumentFragment',
    'CharacterData', 'MutationObserver', 'IntersectionObserver', 'ResizeObserver', 'closed', 'name',
    'MouseEvent', 'KeyboardEvent', 'FocusEvent', 'InputEvent', 'UIEvent', 'PointerEvent']) {
    delete global[k];
  }

  class MessageEvent extends D.Event {
    constructor(type, init) {
      super(type, init);
      init = init || {};
      this.data = init.data === undefined ? null : init.data;
      this.origin = init.origin || '';
      this.lastEventId = '';
      this.source = null;
      this.ports = [];
    }
  }
  class ErrorEvent extends D.Event {
    constructor(type, init) {
      super(type, init);
      init = init || {};
      Object.assign(this, { message: init.message || '', filename: init.filename || '', lineno: 0, colno: 0, error: init.error || null });
    }
  }
  function WorkerGlobalScope() { throw new TypeError('Illegal constructor'); }
  WorkerGlobalScope.prototype = Object.create(global.EventTarget.prototype);
  function DedicatedWorkerGlobalScope() { throw new TypeError('Illegal constructor'); }
  DedicatedWorkerGlobalScope.prototype = Object.create(WorkerGlobalScope.prototype);
  try { Object.setPrototypeOf(global, DedicatedWorkerGlobalScope.prototype); } catch (e) { /* keep the plain global */ }

  // The address of the worker's script, read-only.
  const href = { url: 'about:blank' };
  const loc = {};
  for (const k of ['href', 'protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash', 'origin']) {
    Object.defineProperty(loc, k, {
      enumerable: true,
      get() {
        if (k === 'href') return href.url;
        const p = D.splitURL(href.url);
        if (!p) return '';
        return k === 'origin' ? p.protocol + '//' + p.host : p[k];
      },
    });
  }
  loc.toString = () => href.url;

  let closed = false;
  let name = '';
  for (const [k, v] of Object.entries({
    MessageEvent, ErrorEvent, WorkerGlobalScope, DedicatedWorkerGlobalScope,
    postMessage(data, transfer) {
      void transfer;
      if (!closed) send('M' + kjs.packMessage(data));
    },
    close() {
      if (closed) return;
      closed = true;
      send('Z');
    },
    importScripts() {
      throw new D.DOMException('importScripts is not supported by Kitsune workers yet', 'NotSupportedError');
    },
  })) define(k, v);
  Object.defineProperty(global, 'location', { get: () => loc, configurable: true });
  Object.defineProperty(global, 'name', { get: () => name, configurable: true });
  global.onmessage = null;
  global.onerror = null;
  global.onmessageerror = null;

  const baseOnMessage = kjs.onMessage;
  kjs.onMessage = function (text) {
    const t = /^\{"t":"(\w+)"/.exec(text.slice(0, 16));
    if (!t || (t[1] !== 'winit' && t[1] !== 'wmsg')) { baseOnMessage(text); return; }
    const m = JSON.parse(text);
    if (m.t === 'winit') {
      D.tasks.push({
        name: 'worker ' + m.url,
        run() {
          href.url = String(m.url);
          D.location._url = href.url;
          name = String(m.name || '');
          if (!native.evalScript(String(m.code), String(m.url))) send('E' + 'Uncaught error in ' + m.url);
        },
      });
      return;
    }
    D.tasks.push({
      name: 'message',
      run() {
        if (closed) return;
        let data;
        try { data = kjs.unpackMessage(String(m.data)); } catch (e) {
          D.dispatch(global, new MessageEvent('messageerror'));
          return;
        }
        const ev = new MessageEvent('message', { data });
        D.dispatch(global, ev);
        if (typeof global.onmessage === 'function') {
          try { global.onmessage(ev); } catch (e) {
            console.error('Uncaught ' + String(e) + (e && e.stack ? '\n' + e.stack : ''));
            send('E' + String(e));
          }
        }
      },
    });
  };
})(globalThis, globalThis.__kjs_native, globalThis.__kjs);
