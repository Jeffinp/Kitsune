// Kitsune jsrun --page: observers.
//
// `MutationObserver` sees the changes dom.js makes to the tree (the `mutators` hook): records are
// queued and delivered in a microtask, as in browsers. `IntersectionObserver` and `ResizeObserver`
// work from the layout snapshot of layoutapi.js: they are checked after each snapshot and each scroll
// and their callbacks run as tasks. `PerformanceObserver` sees `performance.mark`/`measure`.
(function (global, native, kjs) {
  'use strict';
  const D = kjs.dom;
  const L = kjs.layout;
  const define = (name, value) => {
    Object.defineProperty(global, name, { value, writable: true, configurable: true, enumerable: false });
  };
  const later = (f) => global.setTimeout(f, 0);

  // ---- MutationObserver ----

  const registrations = new WeakMap(); // node -> [{ observer, options }]
  let observerCount = 0;
  class MutationRecord {
    constructor(type, target) {
      this.type = type; this.target = target;
      this.addedNodes = []; this.removedNodes = [];
      this.previousSibling = null; this.nextSibling = null;
      this.attributeName = null; this.attributeNamespace = null; this.oldValue = null;
    }
  }
  class MutationObserver {
    constructor(callback) {
      if (typeof callback !== 'function') throw new TypeError("MutationObserver: callback is not a function");
      this._cb = callback; this._records = []; this._nodes = []; this._queued = false;
    }
    observe(target, options) {
      if (!target || typeof target.nodeType !== 'number') throw new TypeError("MutationObserver.observe: target is not a Node");
      const o = Object.assign({}, options);
      if (o.attributeOldValue || o.attributeFilter) o.attributes = o.attributes !== false;
      if (o.characterDataOldValue) o.characterData = o.characterData !== false;
      if (!o.childList && !o.attributes && !o.characterData) throw new TypeError("MutationObserver.observe: one of childList, attributes or characterData must be true");
      let list = registrations.get(target);
      if (!list) { list = []; registrations.set(target, list); }
      const old = list.find((r) => r.observer === this);
      if (old) old.options = o;
      else { list.push({ observer: this, options: o }); this._nodes.push(target); observerCount++; }
    }
    disconnect() {
      for (const n of this._nodes) {
        const list = registrations.get(n);
        if (!list) continue;
        const i = list.findIndex((r) => r.observer === this);
        if (i >= 0) { list.splice(i, 1); observerCount--; }
      }
      this._nodes = [];
      this._records = [];
    }
    takeRecords() { const r = this._records; this._records = []; return r; }
    _queue(rec) {
      this._records.push(rec);
      if (this._queued) return;
      this._queued = true;
      Promise.resolve().then(() => {
        this._queued = false;
        const recs = this.takeRecords();
        if (!recs.length) return;
        try { this._cb.call(this, recs, this); } catch (e) { D.reportError(e); }
      });
    }
  }
  function onMutation(type, target, a, b, c, d) {
    if (!observerCount) return;
    const seen = new Set();
    for (let n = target, depth = 0; n && depth < 512; n = n.parentNode || n._kjsHost || null, depth++) {
      const list = registrations.get(n);
      if (!list) continue;
      for (const { observer, options: o } of list) {
        if (seen.has(observer)) continue;
        if (n !== target && !o.subtree) continue;
        let rec = null;
        if (type === 'childList' && o.childList) {
          rec = new MutationRecord('childList', target);
          rec.addedNodes = a; rec.removedNodes = b; rec.previousSibling = c || null; rec.nextSibling = d || null;
        } else if (type === 'attributes' && o.attributes) {
          if (o.attributeFilter && !o.attributeFilter.includes(a)) continue;
          rec = new MutationRecord('attributes', target);
          rec.attributeName = a;
          if (o.attributeOldValue) rec.oldValue = b;
        } else if (type === 'characterData' && o.characterData) {
          rec = new MutationRecord('characterData', target);
          if (o.characterDataOldValue) rec.oldValue = a;
        }
        if (rec) { seen.add(observer); observer._queue(rec); }
      }
    }
  }
  D.mutators.push(onMutation);
  define('MutationObserver', MutationObserver);
  define('WebKitMutationObserver', MutationObserver);
  define('MutationRecord', MutationRecord);

  // ---- IntersectionObserver ----

  const intersections = new Set();
  function margins(text, w, h) {
    const parts = String(text || '0px').trim().split(/\s+/).map((p) => {
      const m = /^(-?[\d.]+)(px|%)?$/.exec(p);
      return m ? { v: parseFloat(m[1]), pct: m[2] === '%' } : { v: 0, pct: false };
    });
    while (parts.length < 4) parts.push(parts[parts.length === 3 ? 1 : parts.length === 2 ? 0 : 0]);
    const val = (p, base) => (p.pct ? (p.v * base) / 100 : p.v);
    return [val(parts[0], h), val(parts[1], w), val(parts[2], h), val(parts[3], w)];
  }
  class IntersectionObserverEntry {
    constructor(o) { Object.assign(this, o); }
  }
  class IntersectionObserver {
    constructor(callback, options) {
      if (typeof callback !== 'function') throw new TypeError('IntersectionObserver: callback is not a function');
      options = options || {};
      this._cb = callback;
      this.root = options.root || null;
      this.rootMargin = options.rootMargin || '0px 0px 0px 0px';
      const t = options.threshold === undefined ? [0] : [].concat(options.threshold);
      this.thresholds = t.map(Number).sort((a, b) => a - b);
      this.scrollMargin = '0px';
      this.delay = 0;
      this.trackVisibility = false;
      this._targets = new Map(); // element -> last [index, intersecting]
      kjs.want('scroll');
    }
    observe(el) {
      if (!el || el.nodeType !== 1 || this._targets.has(el)) return;
      this._targets.set(el, null);
      intersections.add(this);
      schedule();
    }
    unobserve(el) { this._targets.delete(el); if (!this._targets.size) intersections.delete(this); }
    disconnect() { this._targets.clear(); intersections.delete(this); }
    takeRecords() { return []; }
    _check() {
      const v = L.view;
      let root;
      if (this.root && this.root.nodeType === 1 && this.root !== D.document.documentElement) {
        const r = L.rectOf(this.root);
        root = r ? [r[0] - v.sx, r[1] - v.sy, r[2], r[3]] : [0, 0, 0, 0];
      } else root = [0, 0, v.w, v.h];
      const m = margins(this.rootMargin, root[2], root[3]);
      const rx0 = root[0] - m[3], ry0 = root[1] - m[0], rx1 = root[0] + root[2] + m[1], ry1 = root[1] + root[3] + m[2];
      const entries = [];
      for (const [el, last] of this._targets) {
        const r = el.isConnected ? L.rectOf(el) : null;
        const b = r ? [r[0] - v.sx, r[1] - v.sy, r[2], r[3]] : [0, 0, 0, 0];
        const ix0 = Math.max(b[0], rx0), iy0 = Math.max(b[1], ry0);
        const ix1 = Math.min(b[0] + b[2], rx1), iy1 = Math.min(b[1] + b[3], ry1);
        const hit = !!r && ix1 >= ix0 && iy1 >= iy0;
        const area = b[2] * b[3];
        const inter = hit ? Math.max(0, ix1 - ix0) * Math.max(0, iy1 - iy0) : 0;
        const ratio = hit ? (area > 0 ? inter / area : 1) : 0;
        let idx = -1;
        for (let i = 0; i < this.thresholds.length; i++) if (ratio >= this.thresholds[i] && (hit || this.thresholds[i] > 0)) idx = i;
        const state = [idx, hit];
        if (last && last[0] === idx && last[1] === hit) continue;
        this._targets.set(el, state);
        const rect = (q) => new global.DOMRect(q[0], q[1], q[2], q[3]);
        entries.push(new IntersectionObserverEntry({
          target: el, time: performance.now(), isIntersecting: hit, intersectionRatio: ratio,
          boundingClientRect: rect(b), intersectionRect: rect(hit ? [ix0, iy0, ix1 - ix0, iy1 - iy0] : [0, 0, 0, 0]),
          rootBounds: rect([rx0, ry0, rx1 - rx0, ry1 - ry0]), isVisible: hit,
        }));
      }
      if (entries.length) { try { this._cb.call(this, entries, this); } catch (e) { D.reportError(e); } }
    }
  }

  // ---- ResizeObserver ----

  const resizers = new Set();
  class ResizeObserverEntry {
    constructor(o) { Object.assign(this, o); }
  }
  class ResizeObserver {
    constructor(callback) {
      if (typeof callback !== 'function') throw new TypeError('ResizeObserver: callback is not a function');
      this._cb = callback;
      this._targets = new Map(); // element -> last "w h"
    }
    observe(el) {
      if (!el || el.nodeType !== 1) return;
      this._targets.set(el, null);
      resizers.add(this);
      schedule();
    }
    unobserve(el) { this._targets.delete(el); if (!this._targets.size) resizers.delete(this); }
    disconnect() { this._targets.clear(); resizers.delete(this); }
    _check() {
      const entries = [];
      for (const [el, last] of this._targets) {
        const st = el.isConnected ? global.getComputedStyle(el) : null;
        const w = st ? parseFloat(st.width) || 0 : 0;
        const h = st ? parseFloat(st.height) || 0 : 0;
        const r = L.rectOf(el);
        const key = w + ' ' + h;
        if (key === last) continue;
        this._targets.set(el, key);
        const pl = st ? parseFloat(st.paddingLeft) || 0 : 0, pt = st ? parseFloat(st.paddingTop) || 0 : 0;
        entries.push(new ResizeObserverEntry({
          target: el, contentRect: new global.DOMRectReadOnly(pl, pt, w, h),
          contentBoxSize: [{ inlineSize: w, blockSize: h }],
          borderBoxSize: [{ inlineSize: r ? r[2] : 0, blockSize: r ? r[3] : 0 }],
          devicePixelContentBoxSize: [{ inlineSize: w, blockSize: h }],
        }));
      }
      if (entries.length) { try { this._cb.call(this, entries, this); } catch (e) { D.reportError(e); } }
    }
  }

  // Both are checked after a layout and a scroll, in a task of their own.
  let pending = false;
  function schedule() {
    if (pending) return;
    pending = true;
    later(() => {
      pending = false;
      for (const o of [...resizers]) o._check();
      for (const o of [...intersections]) o._check();
    });
  }
  L.onUpdate(schedule);
  define('IntersectionObserver', IntersectionObserver);
  define('IntersectionObserverEntry', IntersectionObserverEntry);
  define('ResizeObserver', ResizeObserver);
  define('ResizeObserverEntry', ResizeObserverEntry);

  // ---- idle callbacks ----

  let idleSeq = 0;
  const idle = new Map();
  define('requestIdleCallback', function (fn, opts) {
    const id = ++idleSeq;
    const start = performance.now();
    const timeout = opts && opts.timeout > 0 ? opts.timeout : 0;
    const t = global.setTimeout(() => {
      idle.delete(id);
      const end = performance.now() + 50;
      fn({ didTimeout: !!timeout && performance.now() - start >= timeout, timeRemaining: () => Math.max(0, end - performance.now()) });
    }, 1);
    idle.set(id, t);
    return id;
  });
  define('cancelIdleCallback', (id) => { const t = idle.get(id); if (t) { global.clearTimeout(t); idle.delete(id); } });

  // ---- performance ----

  const perfObservers = new Set();
  class PerformanceObserver {
    constructor(cb) { this._cb = cb; this._types = []; }
    observe(o) { this._types = o && (o.entryTypes || (o.type ? [o.type] : [])) || []; perfObservers.add(this); }
    disconnect() { perfObservers.delete(this); }
    takeRecords() { return []; }
  }
  PerformanceObserver.supportedEntryTypes = ['mark', 'measure', 'navigation'];
  for (const k of ['mark', 'measure']) {
    const f = performance[k];
    performance[k] = function (...a) {
      const e = f.apply(this, a);
      for (const o of perfObservers) {
        if (!o._types.includes(e.entryType)) continue;
        const list = { getEntries: () => [e], getEntriesByType: (t) => (t === e.entryType ? [e] : []), getEntriesByName: (n) => (n === e.name ? [e] : []) };
        later(() => { try { o._cb(list, o); } catch (err) { D.reportError(err); } });
      }
      return e;
    };
  }
  const byType = performance.getEntriesByType;
  const navEntry = { name: '', entryType: 'navigation', startTime: 0, duration: 0, type: 'navigate', redirectCount: 0, domInteractive: 0, domContentLoadedEventEnd: 0, loadEventEnd: 0, responseStart: 0, transferSize: 0 };
  performance.getEntriesByType = function (t) { if (t === 'navigation') { navEntry.name = location.href; return [navEntry]; } return byType.call(this, t); };
  performance.timing = { navigationStart: performance.timeOrigin, fetchStart: performance.timeOrigin, responseStart: performance.timeOrigin, domLoading: performance.timeOrigin };
  performance.navigation = { type: 0, redirectCount: 0 };
  performance.memory = { usedJSHeapSize: 0, totalJSHeapSize: 0, jsHeapSizeLimit: 48 << 20 };
  define('PerformanceObserver', PerformanceObserver);
})(globalThis, globalThis.__kjs_native, globalThis.__kjs);
