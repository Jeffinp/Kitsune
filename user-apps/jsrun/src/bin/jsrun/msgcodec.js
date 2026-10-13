// Kitsune jsrun: values that travel between a page and its workers (`postMessage`), as text.
//
// The structured clone of a value, written as JSON with tagged objects for what JSON lacks:
// `undefined`, NaN and the infinities, -0, BigInt, Date, RegExp, Map, Set, ArrayBuffer, the typed
// arrays and DataView, Error, Blob and File, and shared references (a value seen twice is written once
// and pointed to). Functions, DOM nodes and symbols cannot be sent (`DataCloneError`, as in browsers).
// Loaded in both the page's prelude and a worker's; `kjs.packMessage` / `kjs.unpackMessage`.
(function (global, native, kjs) {
  'use strict';
  const TYPED = ['Int8Array', 'Uint8Array', 'Uint8ClampedArray', 'Int16Array', 'Uint16Array', 'Int32Array',
    'Uint32Array', 'Float32Array', 'Float64Array', 'BigInt64Array', 'BigUint64Array'];
  const b64 = (u8) => {
    let s = '';
    for (let i = 0; i < u8.length; i += 8192) s += String.fromCharCode.apply(null, u8.subarray(i, i + 8192));
    return global.btoa(s);
  };
  const unb64 = (s) => {
    const t = global.atob(s);
    const u8 = new Uint8Array(t.length);
    for (let i = 0; i < t.length; i++) u8[i] = t.charCodeAt(i);
    return u8;
  };
  const fail = (what) => { throw new DOMException(what + ' could not be cloned.', 'DataCloneError'); };

  function pack(value) {
    const seen = new Map();
    let next = 0;
    const enc = (v) => {
      switch (typeof v) {
        case 'undefined': return { $: 'u' };
        case 'number':
          if (Number.isNaN(v)) return { $: 'n', v: 'NaN' };
          if (!Number.isFinite(v)) return { $: 'n', v: v > 0 ? 'Infinity' : '-Infinity' };
          if (Object.is(v, -0)) return { $: 'n', v: '-0' };
          return v;
        case 'bigint': return { $: 'b', v: v.toString() };
        case 'string': case 'boolean': return v;
        case 'function': case 'symbol': return fail(typeof v);
        default: break;
      }
      if (v === null) return null;
      if (seen.has(v)) return { $: 'r', i: seen.get(v) };
      const id = next++;
      seen.set(v, id);
      if (typeof Node !== 'undefined' && v instanceof Node) fail('a DOM node');
      if (v instanceof Date) return { $: 'd', i: id, v: v.getTime() };
      if (v instanceof RegExp) return { $: 'x', i: id, s: v.source, f: v.flags };
      if (v instanceof ArrayBuffer) return { $: 'ab', i: id, v: b64(new Uint8Array(v)) };
      if (ArrayBuffer.isView(v)) {
        const name = v instanceof DataView ? 'DataView' : v.constructor.name;
        return { $: 'ta', i: id, t: name, v: b64(new Uint8Array(v.buffer, v.byteOffset, v.byteLength)) };
      }
      if (v instanceof Map) return { $: 'm', i: id, v: [...v].map(([k, x]) => [enc(k), enc(x)]) };
      if (v instanceof Set) return { $: 's', i: id, v: [...v].map(enc) };
      if (v instanceof Error) return { $: 'e', i: id, n: v.name, m: v.message, s: v.stack || '' };
      if (typeof Blob !== 'undefined' && v instanceof Blob) {
        return { $: 'bl', i: id, t: v.type, v: b64(v._bytes), n: v.name, l: v.lastModified };
      }
      if (v instanceof Boolean || v instanceof Number || v instanceof String) return { $: 'w', i: id, v: v.valueOf() };
      if (Array.isArray(v)) {
        const out = new Array(v.length);
        for (let k = 0; k < v.length; k++) out[k] = k in v ? enc(v[k]) : { $: 'h' };
        return { $: 'a', i: id, v: out };
      }
      if (v instanceof Promise || v instanceof WeakMap || v instanceof WeakSet) fail(Object.prototype.toString.call(v));
      const o = {};
      for (const k of Object.keys(v)) o[k] = enc(v[k]);
      return { $: 'o', i: id, v: o };
    };
    return JSON.stringify(enc(value));
  }

  function unpack(text) {
    const refs = new Map();
    const dec = (v) => {
      if (v === null || typeof v !== 'object') return v;
      const keep = (x) => { if (v.i !== undefined) refs.set(v.i, x); return x; };
      switch (v.$) {
        case 'u': return undefined;
        case 'n': return v.v === '-0' ? -0 : Number(v.v);
        case 'b': return BigInt(v.v);
        case 'r': return refs.get(v.i);
        case 'h': return undefined;
        case 'd': return keep(new Date(v.v));
        case 'x': return keep(new RegExp(v.s, v.f));
        case 'ab': return keep(unb64(v.v).buffer);
        case 'ta': {
          const bytes = unb64(v.v);
          if (v.t === 'DataView') return keep(new DataView(bytes.buffer));
          const C = TYPED.includes(v.t) ? global[v.t] : Uint8Array;
          return keep(new C(bytes.buffer, 0, bytes.byteLength / (C.BYTES_PER_ELEMENT || 1)));
        }
        case 'm': { const m = keep(new Map()); for (const [k, x] of v.v) m.set(dec(k), dec(x)); return m; }
        case 's': { const s = keep(new Set()); for (const x of v.v) s.add(dec(x)); return s; }
        case 'e': { const C = global[v.n] && global[v.n].prototype instanceof Error ? global[v.n] : Error; const e = keep(new C(v.m)); e.stack = v.s; return e; }
        case 'bl': {
          const parts = [unb64(v.v)];
          return keep(v.n !== undefined && typeof File !== 'undefined' ? new File(parts, v.n, { type: v.t, lastModified: v.l }) : new Blob(parts, { type: v.t }));
        }
        case 'w': return keep(Object(v.v));
        case 'a': { const a = keep(new Array(v.v.length)); v.v.forEach((x, k) => { if (!(x && x.$ === 'h')) a[k] = dec(x); }); return a; }
        case 'o': { const o = keep({}); for (const k of Object.keys(v.v)) o[k] = dec(v.v[k]); return o; }
        default: return v;
      }
    };
    return dec(JSON.parse(text));
  }

  kjs.packMessage = pack;
  kjs.unpackMessage = unpack;
})(globalThis, globalThis.__kjs_native, globalThis.__kjs);
