// Kitsune jsrun --page: `Worker`, the page's side.
//
// `new Worker(url)` fetches the script (same origin, or a blob:/data: address) with `fetch` and asks
// the browser for a process of its own (`K` + `new`): another `jsrun`, started with `--worker`, that
// runs it (workerscope.js). `postMessage` sends the structured clone as text (msgcodec.js) through the
// browser (`K` + `msg`), which hands it to the worker, and back (`worker` messages: `message`,
// `error`). At most MAX_WORKERS run for a page; one more gets an `error` event and does not start.
// `terminate()` ends the process (`K` + `end`). Messages: kitsune_core::web::script::worker.
(function (global, native, kjs) {
  'use strict';
  const D = kjs.dom;
  const send = native.send;
  const MAX_WORKERS = 2;
  const workers = new Map();
  let seq = 0;
  const later = (f) => global.setTimeout(f, 0);

  class Worker extends global.EventTarget {
    constructor(url, opts) {
      super();
      opts = opts || {};
      if (opts.type === 'module') throw new TypeError('Kitsune: module workers are not supported');
      this.onmessage = null;
      this.onerror = null;
      this.onmessageerror = null;
      this._id = ++seq;
      this._queue = [];
      this._state = 'loading';
      const abs = D.resolveURL(String(url), location.href);
      const same = /^(blob|data):/i.test(abs) || (new URL(abs).origin === location.origin);
      if (!same) throw new D.DOMException("Script at '" + abs + "' cannot be accessed from origin '" + location.origin + "'.", 'SecurityError');
      const running = [...workers.values()].filter((w) => w._state !== 'ended').length;
      workers.set(this._id, this);
      if (running >= MAX_WORKERS) {
        console.warn('jsrun: at most ' + MAX_WORKERS + ' workers per page; ' + abs + ' was not started');
        later(() => this._fail('too many workers'));
        return;
      }
      global.fetch(abs).then((r) => {
        if (!r.ok) throw new Error('HTTP ' + r.status);
        return r.text();
      }).then((code) => {
        if (this._state !== 'loading') return;
        send('Knew\n' + this._id + '\n' + abs + '\n' + String(opts.name || '') + '\n' + code);
        this._state = 'running';
        for (const m of this._queue.splice(0)) this._post(m);
      }).catch((e) => this._fail('could not load ' + abs + ': ' + e.message));
    }
    postMessage(data, transfer) {
      void transfer;
      if (this._state === 'ended') return;
      const text = kjs.packMessage(data);
      if (this._state === 'loading') this._queue.push(text);
      else this._post(text);
    }
    _post(text) { send('Kmsg\n' + this._id + '\n' + text); }
    terminate() {
      if (this._state === 'running') send('Kend\n' + this._id);
      this._state = 'ended';
      workers.delete(this._id);
    }
    _fail(message) {
      this._state = 'ended';
      workers.delete(this._id);
      const ev = new global.ErrorEvent('error', { message, cancelable: true });
      D.dispatch(this, ev);
      if (typeof this.onerror === 'function') { try { this.onerror(ev); } catch (e) { D.reportError(e); } }
    }
    _deliver(m) {
      if (m.ev === 'message') {
        let data;
        try { data = kjs.unpackMessage(m.data); } catch (e) {
          const bad = new global.MessageEvent('messageerror', {});
          D.dispatch(this, bad);
          if (typeof this.onmessageerror === 'function') this.onmessageerror(bad);
          return;
        }
        const ev = new global.MessageEvent('message', { data, origin: location.origin });
        D.dispatch(this, ev);
        if (typeof this.onmessage === 'function') { try { this.onmessage(ev); } catch (e) { D.reportError(e); } }
      } else if (m.ev === 'error') {
        const ev = new global.ErrorEvent('error', { message: String(m.message || 'error'), cancelable: true });
        D.dispatch(this, ev);
        if (typeof this.onerror === 'function') { try { this.onerror(ev); } catch (e) { D.reportError(e); } }
      } else if (m.ev === 'end') {
        this._state = 'ended';
        workers.delete(this._id);
      }
    }
  }
  kjs.handlers.worker = function (m) {
    const w = workers.get(m.id | 0);
    if (w) w._deliver(m);
  };
  Object.defineProperty(global, 'Worker', { value: Worker, writable: true, configurable: true, enumerable: false });
  Object.defineProperty(global, 'SharedWorker', { value: undefined, writable: true, configurable: true, enumerable: false });
})(globalThis, globalThis.__kjs_native, globalThis.__kjs);
