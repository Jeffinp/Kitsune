// Kitsune jsrun --page: `localStorage` and `sessionStorage`.
//
// `localStorage` is kept by the browser per origin (the file ~/.webstorage, see
// kitsune_core::web::script::storage): the browser hands this origin's items over right after `init`
// (`store` message) and every change goes back as `L` + JSON (`["set", key, value]`, `["del", key]`,
// `["clear"]`). Reads are local. `sessionStorage` lives as long as the page's process (a reload starts
// it empty). Both answer property access (`localStorage.foo`) as browsers do.
(function (global, native, kjs) {
  'use strict';
  const D = kjs.dom;
  const send = native.send;
  // What one origin may keep, in UTF-16 units of keys and values (kitsune_core::...::ORIGIN_QUOTA).
  const QUOTA = 512 * 1024;

  const data = new WeakMap();
  class Storage {
    constructor(persist) {
      const state = { m: new Map(), persist, size: 0 };
      data.set(this, state);
      const proxy = new Proxy(this, {
        get(t, p, r) {
          if (typeof p === 'symbol' || p in t) return Reflect.get(t, p, r);
          const v = data.get(t).m.get(p);
          return v === undefined ? undefined : v;
        },
        set(t, p, v) {
          if (typeof p === 'symbol' || p in Storage.prototype) return Reflect.set(t, p, v);
          t.setItem(p, v);
          return true;
        },
        has(t, p) { return (typeof p === 'string' && data.get(t).m.has(p)) || p in t; },
        deleteProperty(t, p) { if (typeof p === 'string') t.removeItem(p); return true; },
        ownKeys(t) { return [...data.get(t).m.keys()]; },
        getOwnPropertyDescriptor(t, p) {
          const v = typeof p === 'string' ? data.get(t).m.get(p) : undefined;
          return v === undefined ? undefined : { value: v, enumerable: true, configurable: true, writable: true };
        },
      });
      data.set(proxy, state);
      return proxy;
    }
    get length() { return data.get(this).m.size; }
    key(i) { return [...data.get(this).m.keys()][i] ?? null; }
    getItem(k) { const v = data.get(this).m.get(String(k)); return v === undefined ? null : v; }
    setItem(k, v) {
      k = String(k); v = String(v);
      const s = data.get(this);
      const old = s.m.get(k);
      const size = s.size - (old === undefined ? 0 : k.length + old.length) + k.length + v.length;
      if (size > QUOTA) throw new D.DOMException("Setting the value of '" + k + "' exceeded the quota.", 'QuotaExceededError');
      if (old === v) return;
      s.m.set(k, v);
      s.size = size;
      if (s.persist) send('L' + JSON.stringify(['set', k, v]));
    }
    removeItem(k) {
      k = String(k);
      const s = data.get(this);
      const old = s.m.get(k);
      if (old === undefined) return;
      s.m.delete(k);
      s.size -= k.length + old.length;
      if (s.persist) send('L' + JSON.stringify(['del', k]));
    }
    clear() {
      const s = data.get(this);
      if (!s.m.size) return;
      s.m.clear();
      s.size = 0;
      if (s.persist) send('L["clear"]');
    }
  }
  const local = new Storage(true);
  const session = new Storage(false);
  kjs.handlers.store = function (m) {
    const s = data.get(local);
    s.m.clear();
    s.size = 0;
    for (const [k, v] of Object.entries(m.items || {})) { s.m.set(k, String(v)); s.size += k.length + String(v).length; }
  };
  for (const [k, v] of Object.entries({ Storage, localStorage: local, sessionStorage: session })) {
    Object.defineProperty(global, k, { value: v, writable: k === 'Storage', configurable: true, enumerable: k !== 'Storage' });
  }
})(globalThis, globalThis.__kjs_native, globalThis.__kjs);
