// Kitsune jsrun --page: the network of a page's scripts and the small APIs that come with it.
//
// Evaluated right after dom.js (same prelude). `fetch`, `XMLHttpRequest`, `navigator.sendBeacon`,
// `Headers`, `Request`, `Response` (with a streaming `body`), `ReadableStream` (the part pages use),
// `AbortController`, `Blob`, `File`, `FormData`, `TextEncoder`, `TextDecoder`, `crypto.getRandomValues`,
// `crypto.randomUUID`, `structuredClone`, `URL.createObjectURL`.
//
// A request goes to the browser as an `F` message (kitsune_core::web::script::net): the browser asks
// the ad blocker and the fetcher serves it with the same-origin rules, cookies and redirects. The answer
// comes back as a head (JSON), body pieces (binary, handed to __kjs.onData as ArrayBuffers) and an end
// or an error. `X<id>` aborts. `document.cookie` writes are `C<text>`; the browser answers with the
// new value. See docs/design/tls-browser.md §13.
(function (global, native, kjs) {
  'use strict';
  const send = native.send;
  const sendBytes = native.sendBytes;
  const randomBytes = native.random;

  const report = (e) => console.error('Uncaught ' + String(e) + (e && e.stack ? '\n' + e.stack : ''));
  const define = (name, value) => {
    Object.defineProperty(global, name, { value, writable: true, configurable: true, enumerable: false });
  };

  // ---- text ----

  function utf8Encode(s) {
    s = String(s);
    const out = new Uint8Array(s.length * 3);
    let n = 0;
    for (let i = 0; i < s.length; i++) {
      let c = s.charCodeAt(i);
      if (c >= 0xd800 && c <= 0xdbff && i + 1 < s.length) {
        const d = s.charCodeAt(i + 1);
        if (d >= 0xdc00 && d <= 0xdfff) { c = 0x10000 + ((c - 0xd800) << 10) + (d - 0xdc00); i++; } else c = 0xfffd;
      } else if (c >= 0xd800 && c <= 0xdfff) c = 0xfffd;
      if (c < 0x80) out[n++] = c;
      else if (c < 0x800) { out[n++] = 0xc0 | (c >> 6); out[n++] = 0x80 | (c & 63); }
      else if (c < 0x10000) { out[n++] = 0xe0 | (c >> 12); out[n++] = 0x80 | ((c >> 6) & 63); out[n++] = 0x80 | (c & 63); }
      else {
        out[n++] = 0xf0 | (c >> 18); out[n++] = 0x80 | ((c >> 12) & 63);
        out[n++] = 0x80 | ((c >> 6) & 63); out[n++] = 0x80 | (c & 63);
      }
    }
    return out.slice(0, n);
  }

  // Decode UTF-8 from `b` (Uint8Array). Returns [text, bytes used]; with `stream` an incomplete
  // sequence at the end is left for the next call. Runs of ASCII are converted in blocks.
  function utf8Decode(b, stream, fatal) {
    const parts = [];
    let codes = [];
    let i = 0;
    const n = b.length;
    const bad = () => {
      if (fatal) throw new TypeError('The encoded data was not valid utf-8');
      return 0xfffd;
    };
    const flush = () => { if (codes.length) { parts.push(String.fromCodePoint(...codes)); codes = []; } };
    while (i < n) {
      const c = b[i];
      if (c < 0x80) {
        let j = i + 1;
        while (j < n && j - i < 8192 && b[j] < 0x80) j++;
        flush();
        parts.push(String.fromCharCode.apply(null, b.subarray(i, j)));
        i = j;
        continue;
      }
      let need, cp, lo = 0x80, hi = 0xbf;
      if (c >= 0xc2 && c <= 0xdf) { need = 1; cp = c & 0x1f; }
      else if (c >= 0xe0 && c <= 0xef) { need = 2; cp = c & 0xf; if (c === 0xe0) lo = 0xa0; if (c === 0xed) hi = 0x9f; }
      else if (c >= 0xf0 && c <= 0xf4) { need = 3; cp = c & 7; if (c === 0xf0) lo = 0x90; if (c === 0xf4) hi = 0x8f; }
      else { codes.push(bad()); i++; continue; }
      let k = 1;
      for (; k <= need; k++) {
        if (i + k >= n) break;
        const d = b[i + k];
        if (d < (k === 1 ? lo : 0x80) || d > (k === 1 ? hi : 0xbf)) break;
        cp = (cp << 6) | (d & 0x3f);
      }
      if (k > need) { codes.push(cp); i += need + 1; }
      else if (i + k >= n && stream) break; // an incomplete sequence at the end: wait for the rest
      else { codes.push(bad()); i += k; }
      if (codes.length >= 8192) flush();
    }
    flush();
    return [parts.join(''), i];
  }

  const bytesOf = (x) => {
    if (x instanceof Uint8Array) return x;
    if (x instanceof ArrayBuffer) return new Uint8Array(x);
    if (ArrayBuffer.isView(x)) return new Uint8Array(x.buffer, x.byteOffset, x.byteLength);
    throw new TypeError('expected an ArrayBuffer or a view');
  };

  class TextEncoder {
    get encoding() { return 'utf-8'; }
    encode(s = '') { return utf8Encode(s); }
    encodeInto(s, dest) {
      const b = utf8Encode(s);
      const n = Math.min(b.length, dest.length);
      let cut = n;
      while (cut > 0 && cut < b.length && (b[cut] & 0xc0) === 0x80) cut--;
      dest.set(b.subarray(0, cut));
      return { read: utf8Decode(b.subarray(0, cut))[0].length, written: cut };
    }
  }

  const LATIN1 = new Set(['latin1', 'iso-8859-1', 'iso8859-1', 'windows-1252', 'cp1252', 'us-ascii', 'ascii', 'l1']);
  class TextDecoder {
    constructor(label = 'utf-8', opts = {}) {
      const l = String(label).trim().toLowerCase();
      if (l === 'utf-8' || l === 'utf8' || l === 'unicode-1-1-utf-8') this._enc = 'utf-8';
      else if (LATIN1.has(l)) this._enc = 'windows-1252';
      else if (l === 'utf-16le' || l === 'utf-16') this._enc = 'utf-16le';
      else throw new RangeError('The encoding label provided (\'' + label + '\') is invalid.');
      this.fatal = !!opts.fatal;
      this.ignoreBOM = !!opts.ignoreBOM;
      this._rest = null;
      this._bom = false;
    }
    get encoding() { return this._enc; }
    decode(input, opts = {}) {
      let b = input === undefined ? new Uint8Array(0) : bytesOf(input);
      if (this._rest) { const m = new Uint8Array(this._rest.length + b.length); m.set(this._rest); m.set(b, this._rest.length); b = m; this._rest = null; }
      const stream = !!opts.stream;
      let out;
      if (this._enc === 'utf-8') {
        if (!this._bom && !this.ignoreBOM && b[0] === 0xef && b[1] === 0xbb && b[2] === 0xbf) b = b.subarray(3);
        const [s, used] = utf8Decode(b, stream, this.fatal);
        if (used < b.length) this._rest = b.slice(used);
        out = s;
      } else if (this._enc === 'utf-16le') {
        const n = b.length & ~1;
        const codes = [];
        for (let i = 0; i < n; i += 2) codes.push(b[i] | (b[i + 1] << 8));
        if (n < b.length) this._rest = b.slice(n);
        out = '';
        for (let i = 0; i < codes.length; i += 8192) out += String.fromCharCode(...codes.slice(i, i + 8192));
      } else {
        out = '';
        for (let i = 0; i < b.length; i += 8192) out += String.fromCharCode(...b.subarray(i, i + 8192));
      }
      this._bom = stream;
      if (!stream) this._rest = null;
      return out;
    }
  }

  // ---- Blob, File, FormData ----

  function concatBytes(list, total) {
    if (total === undefined) total = list.reduce((a, b) => a + b.length, 0);
    const out = new Uint8Array(total);
    let at = 0;
    for (const b of list) { out.set(b, at); at += b.length; }
    return out;
  }

  class Blob {
    constructor(parts = [], opts = {}) {
      const list = [];
      for (const p of parts) {
        if (p instanceof Blob) list.push(p._bytes);
        else if (p instanceof ArrayBuffer || ArrayBuffer.isView(p)) list.push(bytesOf(p).slice());
        else list.push(utf8Encode(String(p)));
      }
      this._bytes = concatBytes(list);
      const t = String(opts.type || '');
      this._type = /^[\x20-\x7e]*$/.test(t) ? t.toLowerCase() : '';
    }
    get size() { return this._bytes.length; }
    get type() { return this._type; }
    slice(start = 0, end = this.size, type = '') {
      const b = new Blob([], { type });
      b._bytes = this._bytes.slice(start, end);
      return b;
    }
    arrayBuffer() { return Promise.resolve(this._bytes.slice().buffer); }
    bytes() { return Promise.resolve(this._bytes.slice()); }
    text() { return Promise.resolve(utf8Decode(this._bytes)[0]); }
    stream() {
      const bytes = this._bytes.slice();
      return new ReadableStream({ start(c) { if (bytes.length) c.enqueue(bytes); c.close(); } });
    }
    get [Symbol.toStringTag]() { return 'Blob'; }
  }
  class File extends Blob {
    constructor(parts, name, opts = {}) {
      super(parts, opts);
      this.name = String(name);
      this.lastModified = opts.lastModified === undefined ? Date.now() : Number(opts.lastModified);
    }
    get [Symbol.toStringTag]() { return 'File'; }
  }

  class FormData {
    constructor(form) {
      this._list = [];
      if (form && form.querySelectorAll) {
        for (const el of form.querySelectorAll('input[name], select[name], textarea[name]')) {
          const type = (el.getAttribute('type') || '').toLowerCase();
          if (el.disabled || ['submit', 'button', 'reset', 'file', 'image'].includes(type)) continue;
          if ((type === 'checkbox' || type === 'radio') && !el.checked) continue;
          this.append(el.getAttribute('name'), el.value === undefined ? '' : el.value);
        }
      }
    }
    _entry(name, value, filename) {
      name = String(name);
      if (value instanceof Blob) {
        if (!(value instanceof File) || filename !== undefined) {
          value = new File([value], filename !== undefined ? filename : value instanceof File ? value.name : 'blob', { type: value.type });
        }
        return [name, value];
      }
      return [name, String(value)];
    }
    append(n, v, f) { this._list.push(this._entry(n, v, f)); }
    set(n, v, f) {
      const e = this._entry(n, v, f);
      const i = this._list.findIndex((x) => x[0] === e[0]);
      this.delete(e[0]);
      if (i < 0) this._list.push(e); else this._list.splice(i, 0, e);
    }
    get(n) { const e = this._list.find((x) => x[0] === String(n)); return e ? e[1] : null; }
    getAll(n) { return this._list.filter((x) => x[0] === String(n)).map((x) => x[1]); }
    has(n) { return this._list.some((x) => x[0] === String(n)); }
    delete(n) { this._list = this._list.filter((x) => x[0] !== String(n)); }
    forEach(fn, self) { for (const [k, v] of this._list) fn.call(self, v, k, this); }
    keys() { return this._list.map((x) => x[0])[Symbol.iterator](); }
    values() { return this._list.map((x) => x[1])[Symbol.iterator](); }
    entries() { return this._list.map((x) => [x[0], x[1]])[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
  }

  function multipart(fd) {
    const boundary = '----KitsuneFormBoundary' + Math.random().toString(36).slice(2) + Date.now().toString(36);
    const parts = [];
    const esc = (s) => s.replace(/\r\n|\r|\n/g, '\r\n').replace(/"/g, '%22');
    for (const [k, v] of fd._list) {
      let head = '--' + boundary + '\r\nContent-Disposition: form-data; name="' + esc(k) + '"';
      if (v instanceof File) {
        head += '; filename="' + esc(v.name) + '"\r\nContent-Type: ' + (v.type || 'application/octet-stream');
        parts.push(utf8Encode(head + '\r\n\r\n'), v._bytes, utf8Encode('\r\n'));
      } else {
        parts.push(utf8Encode(head + '\r\n\r\n' + v.replace(/\r\n|\r|\n/g, '\r\n') + '\r\n'));
      }
    }
    parts.push(utf8Encode('--' + boundary + '--\r\n'));
    return [concatBytes(parts), 'multipart/form-data; boundary=' + boundary];
  }

  // ---- Headers ----

  const TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
  class Headers {
    constructor(init) {
      this._map = new Map(); // lowercase name -> [name, value]
      if (init instanceof Headers) for (const [k, v] of init) this.append(k, v);
      else if (Array.isArray(init) || (init && typeof init[Symbol.iterator] === 'function')) {
        for (const pair of init) {
          const p = [...pair];
          if (p.length !== 2) throw new TypeError('Headers: each pair needs a name and a value');
          this.append(p[0], p[1]);
        }
      } else if (init && typeof init === 'object') for (const k of Object.keys(init)) this.append(k, init[k]);
    }
    _check(n, v) {
      n = String(n);
      v = String(v).replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, '');
      if (!TOKEN.test(n)) throw new TypeError('Invalid header name: ' + n);
      if (/[\r\n\0]/.test(v)) throw new TypeError('Invalid header value');
      return [n, v];
    }
    append(n, v) {
      [n, v] = this._check(n, v);
      const k = n.toLowerCase();
      const e = this._map.get(k);
      if (e) e[1] = e[1] + ', ' + v; else this._map.set(k, [n, v]);
    }
    set(n, v) { [n, v] = this._check(n, v); this._map.set(n.toLowerCase(), [n, v]); }
    get(n) { const e = this._map.get(String(n).toLowerCase()); return e ? e[1] : null; }
    has(n) { return this._map.has(String(n).toLowerCase()); }
    delete(n) { this._map.delete(String(n).toLowerCase()); }
    getSetCookie() { return []; }
    forEach(fn, self) { for (const [k, v] of this) fn.call(self, v, k, this); }
    *entries() { for (const k of [...this._map.keys()].sort()) yield [k, this._map.get(k)[1]]; }
    *keys() { for (const [k] of this.entries()) yield k; }
    *values() { for (const [, v] of this.entries()) yield v; }
    [Symbol.iterator]() { return this.entries(); }
  }

  // ---- AbortController ----

  class AbortSignal {
    constructor() { this.aborted = false; this.reason = undefined; this.onabort = null; this._ls = []; }
    addEventListener(t, fn, o) { if (t === 'abort' && fn) this._ls.push({ fn, once: !!(o && o.once) }); }
    removeEventListener(t, fn) { this._ls = this._ls.filter((l) => l.fn !== fn); }
    dispatchEvent(ev) {
      if (typeof this.onabort === 'function') { try { this.onabort.call(this, ev); } catch (e) { report(e); } }
      for (const l of this._ls.slice()) {
        if (l.once) this.removeEventListener('abort', l.fn);
        try { if (typeof l.fn === 'function') l.fn.call(this, ev); else l.fn.handleEvent(ev); } catch (e) { report(e); }
      }
      return true;
    }
    throwIfAborted() { if (this.aborted) throw this.reason; }
    _abort(reason) {
      if (this.aborted) return;
      this.aborted = true;
      this.reason = reason === undefined ? new DOMException('signal is aborted without reason', 'AbortError') : reason;
      const ev = new Event('abort');
      this.dispatchEvent(ev);
    }
    static abort(reason) { const s = new AbortSignal(); s._abort(reason); return s; }
    static timeout(ms) {
      const s = new AbortSignal();
      setTimeout(() => s._abort(new DOMException('signal timed out', 'TimeoutError')), ms);
      return s;
    }
    static any(list) {
      const s = new AbortSignal();
      for (const x of list) {
        if (x.aborted) { s._abort(x.reason); break; }
        x.addEventListener('abort', () => s._abort(x.reason), { once: true });
      }
      return s;
    }
  }
  class AbortController {
    constructor() { this.signal = new AbortSignal(); }
    abort(reason) { this.signal._abort(reason); }
  }

  // ---- ReadableStream (the part pages use: a reader over pieces) ----

  class ReadableStreamDefaultController {
    constructor(stream) { this._s = stream; }
    get desiredSize() { return this._s._state === 'readable' ? Math.max(0, 1 - this._s._queue.length) : 0; }
    enqueue(chunk) {
      const s = this._s;
      if (s._state !== 'readable' || s._closeRequested) throw new TypeError('the stream is closed');
      if (s._reads.length) s._reads.shift().resolve({ value: chunk, done: false });
      else s._queue.push(chunk);
    }
    close() {
      const s = this._s;
      if (s._state !== 'readable') return;
      s._closeRequested = true;
      if (!s._queue.length) s._finish();
    }
    error(e) { this._s._fail(e); }
  }

  class ReadableStream {
    constructor(source = {}) {
      this._queue = [];
      this._reads = [];
      this._state = 'readable';
      this._closeRequested = false;
      this._error = undefined;
      this._reader = null;
      this._source = source;
      this._pulling = false;
      this._closedWaiters = [];
      this._c = new ReadableStreamDefaultController(this);
      try {
        const r = source.start && source.start(this._c);
        if (r && typeof r.then === 'function') r.then(() => this._pull(), (e) => this._fail(e));
      } catch (e) { this._fail(e); }
    }
    get locked() { return !!this._reader; }
    _finish() {
      this._state = 'closed';
      for (const r of this._reads.splice(0)) r.resolve({ value: undefined, done: true });
      for (const w of this._closedWaiters.splice(0)) w.resolve();
    }
    _fail(e) {
      if (this._state !== 'readable') return;
      this._state = 'errored';
      this._error = e;
      this._queue = [];
      for (const r of this._reads.splice(0)) r.reject(e);
      for (const w of this._closedWaiters.splice(0)) w.reject(e);
    }
    _pull() {
      if (!this._source.pull || this._pulling || this._state !== 'readable' || this._queue.length) return;
      this._pulling = true;
      Promise.resolve().then(() => this._source.pull(this._c)).then(
        () => { this._pulling = false; if (this._reads.length) this._pull(); },
        (e) => this._fail(e));
    }
    _read() {
      if (this._queue.length) {
        const value = this._queue.shift();
        if (this._closeRequested && !this._queue.length) this._finish();
        return Promise.resolve({ value, done: false });
      }
      if (this._state === 'closed') return Promise.resolve({ value: undefined, done: true });
      if (this._state === 'errored') return Promise.reject(this._error);
      return new Promise((resolve, reject) => { this._reads.push({ resolve, reject }); this._pull(); });
    }
    cancel(reason) {
      if (this._state !== 'readable') return Promise.resolve();
      this._queue = [];
      this._finish();
      try { if (this._source.cancel) this._source.cancel(reason); } catch (e) { report(e); }
      return Promise.resolve();
    }
    getReader(opts) {
      if (opts && opts.mode === 'byob') throw new TypeError('byob readers are not supported');
      if (this._reader) throw new TypeError('ReadableStream is locked');
      const stream = this;
      const reader = {
        read() { return reader._s ? reader._s._read() : Promise.reject(new TypeError('the reader was released')); },
        releaseLock() { if (reader._s) { reader._s._reader = null; reader._s = null; } },
        cancel(reason) { return reader._s ? reader._s.cancel(reason) : Promise.resolve(); },
        get closed() {
          const s = stream;
          if (s._state === 'closed') return Promise.resolve();
          if (s._state === 'errored') return Promise.reject(s._error);
          return new Promise((resolve, reject) => s._closedWaiters.push({ resolve, reject }));
        },
        _s: stream,
      };
      this._reader = reader;
      return reader;
    }
    async *[Symbol.asyncIterator]() {
      const r = this.getReader();
      try {
        for (;;) { const { value, done } = await r.read(); if (done) return; yield value; }
      } finally { r.releaseLock(); }
    }
    values() { return this[Symbol.asyncIterator](); }
    tee() {
      const r = this.getReader();
      let c1, c2;
      const a = new ReadableStream({ start(c) { c1 = c; } });
      const b = new ReadableStream({ start(c) { c2 = c; } });
      (async () => {
        try {
          for (;;) {
            const { value, done } = await r.read();
            if (done) { c1.close(); c2.close(); return; }
            c1.enqueue(value); c2.enqueue(value);
          }
        } catch (e) { c1.error(e); c2.error(e); }
      })();
      return [a, b];
    }
  }

  async function readAll(stream) {
    const r = stream.getReader();
    const list = [];
    let total = 0;
    for (;;) {
      const { value, done } = await r.read();
      if (done) break;
      const b = typeof value === 'string' ? utf8Encode(value) : bytesOf(value);
      list.push(b);
      total += b.length;
    }
    r.releaseLock();
    return list.length === 1 ? list[0] : concatBytes(list, total);
  }

  // ---- bodies ----

  // [bytes or null, content type or null] for a request or response body.
  function extractBody(body) {
    if (body === null || body === undefined) return [null, null];
    if (typeof body === 'string') return [utf8Encode(body), 'text/plain;charset=UTF-8'];
    if (body instanceof URLSearchParams) return [utf8Encode(body.toString()), 'application/x-www-form-urlencoded;charset=UTF-8'];
    if (body instanceof FormData) return multipart(body);
    if (body instanceof Blob) return [body._bytes, body.type || null];
    if (body instanceof ArrayBuffer || ArrayBuffer.isView(body)) return [bytesOf(body).slice(), null];
    if (body instanceof ReadableStream) return [body, null];
    return [utf8Encode(String(body)), 'text/plain;charset=UTF-8'];
  }

  class Body {
    _initBody(body) {
      this._bodyUsed = false;
      if (body instanceof ReadableStream) this._stream = body;
      else if (body === null || body === undefined) this._stream = null;
      else {
        const bytes = body;
        this._stream = new ReadableStream({ start(c) { if (bytes.length) c.enqueue(bytes); c.close(); } });
      }
    }
    get body() { return this._stream; }
    get bodyUsed() { return this._bodyUsed; }
    _consume() {
      if (this._bodyUsed) return Promise.reject(new TypeError('Body has already been consumed.'));
      this._bodyUsed = true;
      if (!this._stream) return Promise.resolve(new Uint8Array(0));
      if (this._stream.locked) return Promise.reject(new TypeError('ReadableStream is locked'));
      return readAll(this._stream);
    }
    arrayBuffer() { return this._consume().then((b) => (b.byteOffset === 0 && b.byteLength === b.buffer.byteLength ? b.buffer : b.slice().buffer)); }
    bytes() { return this._consume().then((b) => new Uint8Array(b)); }
    text() { return this._consume().then((b) => utf8Decode(b)[0]); }
    json() { return this.text().then((t) => JSON.parse(t)); }
    blob() { return this._consume().then((b) => { const x = new Blob([], { type: this.headers.get('content-type') || '' }); x._bytes = b; return x; }); }
    formData() {
      return this.text().then((t) => {
        const fd = new FormData();
        for (const [k, v] of new URLSearchParams(t)) fd.append(k, v);
        return fd;
      });
    }
  }

  const absURL = (u) => (/^(blob|data):/i.test(String(u).trim()) ? String(u).trim() : new URL(String(u), location.href).href);

  class Request extends Body {
    constructor(input, init = {}) {
      super();
      const from = input instanceof Request ? input : null;
      this.url = from ? from.url : absURL(input);
      if (/^https?:\/\/[^/]*@/i.test(this.url)) throw new TypeError('Request cannot be constructed from a URL that includes credentials');
      const m = init.method !== undefined ? String(init.method) : from ? from.method : 'GET';
      if (!TOKEN.test(m)) throw new TypeError('Invalid method');
      const up = m.toUpperCase();
      this.method = ['DELETE', 'GET', 'HEAD', 'OPTIONS', 'POST', 'PUT', 'PATCH'].includes(up) ? up : m;
      if (['CONNECT', 'TRACE', 'TRACK'].includes(up)) throw new TypeError("'" + m + "' HTTP method is unsupported.");
      this.headers = new Headers(init.headers !== undefined ? init.headers : from ? from.headers : undefined);
      this.mode = init.mode || (from ? from.mode : 'cors');
      this.credentials = init.credentials || (from ? from.credentials : 'same-origin');
      this.redirect = init.redirect || (from ? from.redirect : 'follow');
      // Only the modes of the standard (the browser also knows `script`, for loadScript alone).
      if (!['cors', 'no-cors', 'same-origin', 'navigate'].includes(this.mode)) throw new TypeError("Failed to construct 'Request': invalid mode '" + this.mode + "'");
      if (!['omit', 'same-origin', 'include'].includes(this.credentials)) throw new TypeError("Failed to construct 'Request': invalid credentials '" + this.credentials + "'");
      if (!['follow', 'error', 'manual'].includes(this.redirect)) throw new TypeError("Failed to construct 'Request': invalid redirect '" + this.redirect + "'");
      this.cache = init.cache || (from ? from.cache : 'default');
      this.referrer = 'about:client';
      this.referrerPolicy = init.referrerPolicy || '';
      this.integrity = init.integrity || '';
      this.keepalive = !!init.keepalive;
      this.destination = '';
      this.signal = init.signal || (from ? from.signal : new AbortSignal());
      let body = init.body !== undefined ? init.body : null;
      if (body === null && from && from._raw) body = from._raw;
      if (body !== null && (this.method === 'GET' || this.method === 'HEAD')) throw new TypeError('Request with GET/HEAD method cannot have body.');
      const [bytes, type] = extractBody(body);
      if (type && !this.headers.has('content-type')) this.headers.set('content-type', type);
      this._raw = bytes;
      this._initBody(bytes);
    }
    clone() { return new Request(this); }
  }

  const NULL_BODY = new Set([101, 103, 204, 205, 304]);
  class Response extends Body {
    constructor(body = null, init = {}) {
      super();
      const status = init.status === undefined ? 200 : init.status | 0;
      if (status < 200 || status > 599) throw new RangeError('The status provided (' + status + ') is outside the range [200, 599].');
      this.status = status;
      this.statusText = init.statusText === undefined ? '' : String(init.statusText);
      this.headers = new Headers(init.headers);
      this.type = 'default';
      this.url = '';
      this.redirected = false;
      const [bytes, type] = extractBody(body);
      if (bytes !== null && NULL_BODY.has(status)) throw new TypeError('Response with null body status cannot have body');
      if (type && !this.headers.has('content-type')) this.headers.set('content-type', type);
      this._initBody(bytes);
    }
    get ok() { return this.status >= 200 && this.status <= 299; }
    clone() {
      if (this._bodyUsed) throw new TypeError('Response body is already used');
      const r = Object.create(Response.prototype);
      Object.assign(r, this);
      r.headers = new Headers(this.headers);
      if (this._stream) { const [a, b] = this._stream.tee(); this._stream = a; r._stream = b; }
      return r;
    }
    static error() { const r = new Response(null, { status: 200 }); r.status = 0; r.type = 'error'; return r; }
    static json(data, init = {}) {
      const h = new Headers(init.headers);
      if (!h.has('content-type')) h.set('content-type', 'application/json');
      return new Response(JSON.stringify(data), { ...init, headers: h });
    }
    static redirect(url, status = 302) {
      const r = new Response(null, { status, headers: { location: absURL(url) } });
      return r;
    }
  }

  // ---- the requests in flight ----

  let nextId = 1;
  const pending = new Map(); // id -> { head(m), data(buffer), end(), error(msg) }

  // Send request `req` (a Request) and answer through `rec`; returns the id.
  function startRequest(req, rec) {
    const id = nextId++;
    let body = req._raw;
    if (body instanceof ReadableStream) {
      rec.error('streaming request bodies are not supported');
      return id;
    }
    pending.set(id, rec);
    let head = 'F' + id + '\n' + req.method + '\n' + req.url + '\n' + req.mode + '\n' + req.credentials + '\n' +
      req.redirect + '\n' + (body ? '1' : '0') + '\n';
    for (const [k, v] of req.headers) head += k + ': ' + v + '\n';
    head += '\n';
    let ab = null;
    if (body && body.length) ab = body.byteOffset === 0 && body.byteLength === body.buffer.byteLength ? body.buffer : body.slice().buffer;
    const r = sendBytes(head, ab);
    if (r < 0) {
      pending.delete(id);
      Promise.resolve().then(() => rec.error(r === -7 ? 'request body too large' : 'could not send the request'));
    }
    return id;
  }
  function abortRequest(id) {
    if (pending.delete(id)) send('X' + id);
  }

  function onNet(m) {
    const rec = pending.get(m.id);
    if (!rec) return;
    if (m.ev === 'head') {
      if (typeof m.cookie === 'string') kjs.dom.document._cookieText = m.cookie;
      rec.head(m);
    } else if (m.ev === 'end') { pending.delete(m.id); rec.end(); }
    else if (m.ev === 'error') { pending.delete(m.id); rec.error(String(m.message || 'network error')); }
  }

  const baseOnMessage = kjs.onMessage;
  kjs.onMessage = function (text) {
    if (text.startsWith('{"t":"fetch"')) { onNet(JSON.parse(text)); return; }
    if (text.startsWith('{"t":"cookie"')) { kjs.dom.document._cookieText = String(JSON.parse(text).v); return; }
    baseOnMessage(text);
  };
  kjs.onData = function (id, buffer) {
    const rec = pending.get(id);
    if (rec) rec.data(buffer);
  };

  function headersFrom(list) {
    const h = new Headers();
    for (const [k, v] of list || []) { try { h.append(k, v); } catch (e) { /* skip a bad one */ } }
    return h;
  }

  // ---- fetch ----

  function fetch(input, init) {
    let req;
    try { req = new Request(input, init); } catch (e) { return Promise.reject(e); }
    const signal = req.signal;
    if (signal && signal.aborted) return Promise.reject(signal.reason);
    if (/^blob:/i.test(req.url)) return fetchBlob(req);
    if (/^data:/i.test(req.url)) return fetchData(req);
    if (!/^https?:/i.test(req.url)) return Promise.reject(new TypeError('Failed to fetch: unsupported scheme'));
    return new Promise((resolve, reject) => {
      let controller = null;
      let id = 0;
      const onAbort = () => {
        abortRequest(id);
        const why = signal.reason;
        if (controller) { try { controller.error(why); } catch (e) { /* closed */ } } else reject(why);
      };
      if (signal) signal.addEventListener('abort', onAbort, { once: true });
      const done = () => { if (signal) signal.removeEventListener('abort', onAbort); };
      id = startRequest(req, {
        head(m) {
          const res = Object.create(Response.prototype);
          res.status = m.status;
          res.statusText = m.statusText;
          res.headers = headersFrom(m.headers);
          res.type = m.type;
          res.url = m.url;
          res.redirected = !!m.redirected;
          const noBody = req.method === 'HEAD' || NULL_BODY.has(m.status) || m.type === 'opaque' || m.type === 'opaqueredirect';
          res._initBody(new ReadableStream({
            start(c) { controller = c; if (noBody) { /* closed on end */ } },
            cancel() { abortRequest(id); done(); },
          }));
          resolve(res);
        },
        data(buffer) {
          if (controller) { try { controller.enqueue(new Uint8Array(buffer)); } catch (e) { /* cancelled */ } }
        },
        end() {
          done();
          if (controller) { try { controller.close(); } catch (e) { /* cancelled */ } }
        },
        error(msg) {
          done();
          const e = new TypeError('Failed to fetch: ' + msg);
          if (controller) { try { controller.error(e); } catch (x) { /* closed */ } } else reject(e);
        },
      });
    });
  }

  // ---- blob: and data: addresses ----

  const blobs = new Map();
  function fetchBlob(req) {
    const b = blobs.get(req.url);
    if (!b || req.method !== 'GET') return Promise.reject(new TypeError('Failed to fetch'));
    const r = new Response(b._bytes.slice(), { status: 200, headers: { 'content-type': b.type, 'content-length': String(b.size) } });
    r.url = req.url;
    r.type = 'basic';
    return Promise.resolve(r);
  }
  function fetchData(req) {
    const m = /^data:([^,]*?)(;base64)?,(.*)$/is.exec(req.url);
    if (!m) return Promise.reject(new TypeError('Failed to fetch'));
    let bytes;
    if (m[2]) {
      const bin = atob(decodeURIComponent(m[3]));
      bytes = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    } else bytes = utf8Encode(decodeURIComponent(m[3]));
    const r = new Response(bytes, { status: 200, headers: { 'content-type': m[1] || 'text/plain;charset=US-ASCII' } });
    r.url = req.url;
    r.type = 'basic';
    return Promise.resolve(r);
  }
  URL.createObjectURL = function (blob) {
    if (!(blob instanceof Blob)) throw new TypeError('createObjectURL needs a Blob');
    const u = 'blob:' + location.origin + '/' + crypto.randomUUID();
    blobs.set(u, blob);
    return u;
  };
  URL.revokeObjectURL = function (u) { blobs.delete(String(u)); };
  URL.canParse = function (u, base) { try { new URL(u, base); return true; } catch (e) { return false; } };

  // ---- XMLHttpRequest ----

  const XHR_EVENTS = ['readystatechange', 'loadstart', 'progress', 'abort', 'error', 'load', 'timeout', 'loadend'];
  class XHREventTarget {
    constructor() { this._xls = new Map(); for (const t of XHR_EVENTS) this['on' + t] = null; }
    addEventListener(t, fn) { if (!fn) return; if (!this._xls.has(t)) this._xls.set(t, []); this._xls.get(t).push(fn); }
    removeEventListener(t, fn) { const l = this._xls.get(t); if (l) this._xls.set(t, l.filter((x) => x !== fn)); }
    dispatchEvent(ev) {
      try { Object.defineProperty(ev, 'target', { value: this, configurable: true }); Object.defineProperty(ev, 'currentTarget', { value: this, configurable: true }); } catch (e) { /* frozen */ }
      const h = this['on' + ev.type];
      if (typeof h === 'function') { try { h.call(this, ev); } catch (e) { report(e); } }
      for (const fn of (this._xls.get(ev.type) || []).slice()) {
        try { if (typeof fn === 'function') fn.call(this, ev); else fn.handleEvent(ev); } catch (e) { report(e); }
      }
      return true;
    }
  }
  const progress = (type, loaded, total) => {
    const ev = new Event(type);
    ev.lengthComputable = total > 0;
    ev.loaded = loaded;
    ev.total = total;
    return ev;
  };

  class XMLHttpRequest extends XHREventTarget {
    constructor() {
      super();
      this.readyState = 0;
      this.status = 0;
      this.statusText = '';
      this.responseType = '';
      this.responseURL = '';
      this.timeout = 0;
      this.withCredentials = false;
      this.upload = new XHREventTarget();
      this._headers = new Headers();
      this._resHeaders = new Headers();
      this._chunks = [];
      this._size = 0;
      this._id = 0;
      this._timer = 0;
      this._mime = null;
      this._text = null;
    }
    open(method, url, async = true) {
      if (async === false) console.warn('XMLHttpRequest: synchronous requests are not supported; it runs asynchronously');
      this._abortNow(false);
      this._method = String(method);
      this._url = absURL(url);
      this._headers = new Headers();
      this._resHeaders = new Headers();
      this._chunks = [];
      this._size = 0;
      this._text = null;
      this.status = 0;
      this.statusText = '';
      this.responseURL = '';
      this._send = false;
      this._state(1);
    }
    setRequestHeader(n, v) {
      if (this.readyState !== 1 || this._send) throw new DOMException('The object\'s state must be OPENED.', 'InvalidStateError');
      this._headers.append(n, v);
    }
    overrideMimeType(m) { this._mime = String(m); }
    _state(n) { this.readyState = n; this.dispatchEvent(new Event('readystatechange')); }
    send(body = null) {
      if (this.readyState !== 1 || this._send) throw new DOMException('The object\'s state must be OPENED.', 'InvalidStateError');
      this._send = true;
      const m = this._method.toUpperCase();
      if (m === 'GET' || m === 'HEAD') body = null;
      let req;
      try {
        req = new Request(this._url, {
          method: this._method, headers: this._headers, body,
          credentials: this.withCredentials ? 'include' : 'same-origin',
        });
      } catch (e) {
        Promise.resolve().then(() => this._fail('error'));
        return;
      }
      this.dispatchEvent(progress('loadstart', 0, 0));
      if (this.timeout > 0) this._timer = setTimeout(() => this._abortNow(true, 'timeout'), this.timeout);
      const xhr = this;
      this._id = startRequest(req, {
        head(h) {
          xhr.status = h.type === 'opaque' ? 0 : h.status;
          xhr.statusText = h.statusText;
          xhr.responseURL = h.url;
          xhr._resHeaders = headersFrom(h.headers);
          xhr._total = Number(xhr._resHeaders.get('content-length')) || 0;
          xhr._state(2);
        },
        data(buffer) {
          const b = new Uint8Array(buffer);
          xhr._chunks.push(b);
          xhr._size += b.length;
          xhr._text = null;
          if (xhr.readyState !== 3) xhr._state(3); else xhr.dispatchEvent(new Event('readystatechange'));
          xhr.dispatchEvent(progress('progress', xhr._size, xhr._total));
        },
        end() {
          clearTimeout(xhr._timer);
          xhr._id = 0;
          if (xhr.readyState < 3) xhr._state(3);
          xhr._state(4);
          xhr.dispatchEvent(progress('load', xhr._size, xhr._total));
          xhr.dispatchEvent(progress('loadend', xhr._size, xhr._total));
        },
        error() { clearTimeout(xhr._timer); xhr._id = 0; xhr._fail('error'); },
      });
    }
    _fail(type) {
      this.status = 0;
      this.statusText = '';
      this._chunks = [];
      this._size = 0;
      this._state(4);
      this.dispatchEvent(progress(type, 0, 0));
      this.dispatchEvent(progress('loadend', 0, 0));
    }
    _abortNow(fire, type = 'abort') {
      clearTimeout(this._timer);
      if (this._id) { abortRequest(this._id); this._id = 0; }
      else if (!this._send || this.readyState === 4 || this.readyState === 0) { if (!fire) return; }
      if (fire && this._send && this.readyState !== 4) this._fail(type);
      this._send = false;
    }
    abort() { this._abortNow(true); if (this.readyState === 4) this.readyState = 0; }
    getResponseHeader(n) { return this.readyState >= 2 ? this._resHeaders.get(n) : null; }
    getAllResponseHeaders() {
      if (this.readyState < 2) return '';
      let s = '';
      for (const [k, v] of this._resHeaders) s += k + ': ' + v + '\r\n';
      return s;
    }
    _bytes() {
      if (this._chunks.length > 1) this._chunks = [concatBytes(this._chunks, this._size)];
      return this._chunks[0] || new Uint8Array(0);
    }
    get responseText() {
      if (this.responseType !== '' && this.responseType !== 'text') throw new DOMException('responseText is only for text', 'InvalidStateError');
      if (this.readyState < 3) return '';
      if (this._text === null) this._text = utf8Decode(this._bytes())[0];
      return this._text;
    }
    get response() {
      const t = this.responseType;
      if (t === '' || t === 'text') return this.responseText;
      if (this.readyState !== 4) return null;
      if (t === 'json') { try { return JSON.parse(utf8Decode(this._bytes())[0]); } catch (e) { return null; } }
      if (t === 'arraybuffer') return this._bytes().slice().buffer;
      if (t === 'blob') { const b = new Blob([], { type: this._mime || this._resHeaders.get('content-type') || '' }); b._bytes = this._bytes().slice(); return b; }
      if (t === 'document') return null;
      return null;
    }
    get responseXML() { return null; }
  }
  XMLHttpRequest.UNSENT = 0;
  XMLHttpRequest.OPENED = 1;
  XMLHttpRequest.HEADERS_RECEIVED = 2;
  XMLHttpRequest.LOADING = 3;
  XMLHttpRequest.DONE = 4;

  // ---- crypto, structuredClone, sendBeacon, performance ----

  const crypto = {
    getRandomValues(view) {
      if (!ArrayBuffer.isView(view) || view instanceof Float32Array || view instanceof Float64Array || view instanceof DataView) {
        throw new DOMException('The data is not an integer typed array', 'TypeMismatchError');
      }
      if (view.byteLength > 65536) throw new DOMException('The ArrayBufferView\'s byte length exceeds 65536', 'QuotaExceededError');
      new Uint8Array(view.buffer, view.byteOffset, view.byteLength).set(new Uint8Array(randomBytes(view.byteLength)));
      return view;
    },
    randomUUID() {
      const b = new Uint8Array(randomBytes(16));
      b[6] = (b[6] & 0x0f) | 0x40;
      b[8] = (b[8] & 0x3f) | 0x80;
      const h = [...b].map((x) => x.toString(16).padStart(2, '0')).join('');
      return h.slice(0, 8) + '-' + h.slice(8, 12) + '-' + h.slice(12, 16) + '-' + h.slice(16, 20) + '-' + h.slice(20);
    },
    subtle: undefined,
  };

  function structuredClone(value, opts) {
    const seen = new Map();
    const clone = (v) => {
      if (v === null || (typeof v !== 'object' && typeof v !== 'function' && typeof v !== 'symbol')) return v;
      if (typeof v === 'function' || typeof v === 'symbol') throw new DOMException(String(v) + ' could not be cloned.', 'DataCloneError');
      if (seen.has(v)) return seen.get(v);
      let out;
      if (v instanceof Date) out = new Date(v.getTime());
      else if (v instanceof RegExp) out = new RegExp(v.source, v.flags);
      else if (v instanceof ArrayBuffer) out = v.slice(0);
      else if (ArrayBuffer.isView(v)) {
        const buf = clone(v.buffer);
        out = v instanceof DataView ? new DataView(buf, v.byteOffset, v.byteLength) : new v.constructor(buf, v.byteOffset, v.length);
      } else if (v instanceof Map) { out = new Map(); seen.set(v, out); for (const [k, x] of v) out.set(clone(k), clone(x)); return out; }
      else if (v instanceof Set) { out = new Set(); seen.set(v, out); for (const x of v) out.add(clone(x)); return out; }
      else if (v instanceof Blob) { out = v.slice(0, v.size, v.type); if (v instanceof File) out = new File([out], v.name, { type: v.type, lastModified: v.lastModified }); }
      else if (v instanceof Error) { out = new (global[v.name] || Error)(v.message); seen.set(v, out); return out; }
      else if (v instanceof Boolean || v instanceof Number || v instanceof String) out = Object(v.valueOf());
      else if (Array.isArray(v)) { out = new Array(v.length); seen.set(v, out); for (let i = 0; i < v.length; i++) if (i in v) out[i] = clone(v[i]); return out; }
      else if (v instanceof Node || v instanceof Promise || v instanceof WeakMap || v instanceof WeakSet) {
        throw new DOMException(Object.prototype.toString.call(v) + ' could not be cloned.', 'DataCloneError');
      } else {
        out = {};
        seen.set(v, out);
        for (const k of Object.keys(v)) out[k] = clone(v[k]);
        return out;
      }
      seen.set(v, out);
      return out;
    };
    void opts;
    return clone(value);
  }

  navigator.sendBeacon = function (url, data) {
    try {
      fetch(url, { method: 'POST', body: data === undefined ? null : data, keepalive: true, credentials: 'include', mode: 'no-cors' })
        .catch(() => {});
      return true;
    } catch (e) { return false; }
  };

  const perfStart = Date.now() - performance.now();
  const marks = [];
  Object.assign(performance, {
    timeOrigin: perfStart,
    mark(name) { const e = { name: String(name), entryType: 'mark', startTime: performance.now(), duration: 0 }; marks.push(e); return e; },
    measure(name, a, b) {
      const at = (x) => { const m = marks.filter((e) => e.name === x).pop(); return m ? m.startTime : 0; };
      const s = a ? at(a) : 0;
      const t = b ? at(b) : performance.now();
      const e = { name: String(name), entryType: 'measure', startTime: s, duration: t - s };
      marks.push(e);
      return e;
    },
    getEntriesByName(n) { return marks.filter((e) => e.name === n); },
    getEntriesByType(t) { return marks.filter((e) => e.entryType === t); },
    getEntries() { return marks.slice(); },
    clearMarks() { marks.length = 0; },
    clearMeasures() {},
    now: performance.now,
    toJSON() { return { timeOrigin: perfStart }; },
  });

  // URLSearchParams: what dom.js leaves out.
  const USP = URLSearchParams.prototype;
  if (!('size' in USP)) Object.defineProperty(USP, 'size', { get() { return this._list.length; } });
  // application/x-www-form-urlencoded: spaces become '+', and only *-._ and alphanumerics stay.
  const formEncode = (x) => utf8Encode(String(x)).reduce((out, c) => out + (
    (c >= 0x30 && c <= 0x39) || (c >= 0x41 && c <= 0x5a) || (c >= 0x61 && c <= 0x7a) || c === 0x2a || c === 0x2d || c === 0x2e || c === 0x5f
      ? String.fromCharCode(c) : c === 0x20 ? '+' : '%' + c.toString(16).toUpperCase().padStart(2, '0')), '');
  USP.toString = function () { return this._list.map(([k, v]) => formEncode(k) + '=' + formEncode(v)).join('&'); };
  USP.sort = function () { this._list.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)); };

  // A `<script src>` a script added (scripts.js): mode `script` is the browser's "no-cors, but hand
  // over a JavaScript answer" (kitsune_core::cors: only a JavaScript Content-Type is delivered), with
  // the cookies of the site. The text goes to `done(code)` (null on failure), never to the page.
  function loadScript(url, cors, done) {
    const parts = [];
    let ok = false;
    startRequest({ method: 'GET', url, mode: cors ? 'cors' : 'script', credentials: cors === 'use-credentials' || !cors ? 'include' : 'same-origin',
      redirect: 'follow', headers: [], _raw: null }, {
      head(m) { ok = m.status >= 200 && m.status < 300; },
      data(buffer) { if (ok) parts.push(new Uint8Array(buffer)); },
      end() {
        if (!ok) { done(null); return; }
        let n = 0;
        for (const p of parts) n += p.length;
        const all = new Uint8Array(n);
        n = 0;
        for (const p of parts) { all.set(p, n); n += p.length; }
        done(utf8Decode(all, false, false)[0]);
      },
      error() { done(null); },
    });
  }
  kjs.net = { loadScript };

  for (const [k, v] of Object.entries({
    fetch, Headers, Request, Response, ReadableStream, ReadableStreamDefaultController, AbortController,
    AbortSignal, Blob, File, FormData, TextEncoder, TextDecoder, XMLHttpRequest, structuredClone, crypto,
  })) define(k, v);
})(globalThis, globalThis.__kjs_native, globalThis.__kjs);
