// Kitsune jsrun --page: layout as the page's scripts measure it.
//
// The browser lays the page out (kitsune_core::web::layout) and, after every layout of a page with a
// script process, sends the box of each element and a few computed properties as one binary message
// (`boxes`, the network data tag with id 0; kitsune_core::web::script::boxes_message), with the viewport
// and the scroll. Scripts read that snapshot synchronously: `getBoundingClientRect`, `offset*`,
// `client*`, `scroll*`, `getComputedStyle`, `elementFromPoint`, `innerWidth`... An element added since
// the last layout has no box yet (zeros) until the layout that follows the document it is in. The
// scroll (`scrollTo`, `scrollIntoView`, `scrollTop = `) goes to the browser as `S<x> <y>`; `view`
// messages bring the scroll and the size when they change without a layout (`scroll`, `resize`).
(function (global, native, kjs) {
  'use strict';
  const D = kjs.dom;
  const { document, dispatch, byUid, Element } = D;
  const send = native.send;
  const define = (name, value) => {
    Object.defineProperty(global, name, { value, writable: true, configurable: true, enumerable: false });
  };

  // ---- geometry classes ----

  class DOMRectReadOnly {
    constructor(x, y, w, h) { this._x = +x || 0; this._y = +y || 0; this._w = +w || 0; this._h = +h || 0; }
    get x() { return this._x; } get y() { return this._y; }
    get width() { return this._w; } get height() { return this._h; }
    get left() { return Math.min(this._x, this._x + this._w); } get right() { return Math.max(this._x, this._x + this._w); }
    get top() { return Math.min(this._y, this._y + this._h); } get bottom() { return Math.max(this._y, this._y + this._h); }
    toJSON() { return { x: this.x, y: this.y, width: this.width, height: this.height, top: this.top, right: this.right, bottom: this.bottom, left: this.left }; }
    static fromRect(r) { r = r || {}; return new this(r.x, r.y, r.width, r.height); }
  }
  class DOMRect extends DOMRectReadOnly {
    set x(v) { this._x = +v; } set y(v) { this._y = +v; } set width(v) { this._w = +v; } set height(v) { this._h = +v; }
    get x() { return this._x; } get y() { return this._y; } get width() { return this._w; } get height() { return this._h; }
  }
  class DOMPoint {
    constructor(x, y, z, w) { this.x = +x || 0; this.y = +y || 0; this.z = +z || 0; this.w = w === undefined ? 1 : +w; }
    static fromPoint(p) { p = p || {}; return new DOMPoint(p.x, p.y, p.z, p.w); }
    toJSON() { return { x: this.x, y: this.y, z: this.z, w: this.w }; }
  }
  class DOMMatrix {
    constructor() { Object.assign(this, { a: 1, b: 0, c: 0, d: 1, e: 0, f: 0, is2D: true, isIdentity: true }); }
    translate(x, y) { const m = new DOMMatrix(); Object.assign(m, this); m.e += +x || 0; m.f += +y || 0; m.isIdentity = !m.e && !m.f; return m; }
    scale(s) { const m = new DOMMatrix(); Object.assign(m, this); m.a *= s; m.d *= s; m.isIdentity = false; return m; }
    multiply() { return this; }
    inverse() { return this; }
    transformPoint(p) { return new DOMPoint(p.x * this.a + this.e, p.y * this.d + this.f); }
    toString() { return 'matrix(' + [this.a, this.b, this.c, this.d, this.e, this.f].join(', ') + ')'; }
  }
  for (const [k, v] of Object.entries({ DOMRectReadOnly, DOMRect, DOMPoint, DOMPointReadOnly: DOMPoint, DOMMatrix, DOMMatrixReadOnly: DOMMatrix, WebKitCSSMatrix: DOMMatrix })) define(k, v);

  // ---- the snapshot ----

  const view = { w: global.innerWidth || 0, h: global.innerHeight || 0, sx: 0, sy: 0, pw: 0, ph: 0, z: 100 };
  // uid -> record: [x, y, w, h, color, bg, font|display|flags, m tb, m rl... ] (see `boxes`).
  let boxes = new Map();
  let memo = new Map();
  let generation = 0;
  const listeners = [];

  const DISPLAY = ['none', 'block', 'inline', 'inline-block', 'flex', 'grid', 'table', 'table-row',
    'table-cell', 'list-item', 'inline-flex', 'contents', 'table-row-group', 'table-header-group',
    'table-footer-group', 'table-caption', 'inline-grid'];
  const disp = (rec) => (rec[6] >> 11) & 31;
  const flags = (rec) => rec[6] & 2047;
  const F_BOLD = 1, F_ITALIC = 2, F_HIDDEN = 4, F_ABS = 8, F_FIXED = 16, F_REL = 32, F_STICKY = 64, F_MONO = 128, F_BOX = 256, F_FLOAT_L = 512, F_FLOAT_R = 1024;

  // The binary snapshot: a header of 9 words (version, viewport w and h, scroll x and y, page w and
  // h, zoom, count) and `count` records of 13 words: uid, x, y, w, h (CSS px, page coordinates),
  // colour (0xRRGGBB), background (0xAARRGGBB, 0 = none), font px << 16 | display << 11 | flags,
  // margins (top << 16 | right, bottom << 16 | left), paddings (same), borders (one byte each).
  function takeBoxes(buffer) {
    if (buffer.byteLength < 36) return;
    const w = new Int32Array(buffer, 0, buffer.byteLength >> 2);
    if (w[0] !== 1) return;
    const n = Math.min(w[8] >>> 0, ((w.length - 9) / 13) | 0);
    const map = new Map();
    for (let i = 0, k = 9; i < n; i++, k += 13) map.set(w[k] >>> 0, w.subarray(k + 1, k + 13));
    boxes = map;
    memo = new Map();
    generation++;
    setView(w[1], w[2], w[3], w[4], w[5], w[6], w[7]);
    for (const f of listeners) { try { f(); } catch (e) { D.reportError(e); } }
  }
  const baseOnData = kjs.onData;
  kjs.onData = function (id, buffer) {
    if (id === 0) { D.tasks.push({ name: 'layout', run: () => takeBoxes(buffer) }); return; }
    baseOnData(id, buffer);
  };
  kjs.handlers.view = (m) => setView(m.w, m.h, m.sx, m.sy, m.pw, m.ph, m.z);

  function setView(w, h, sx, sy, pw, ph, z) {
    const resized = w !== view.w || h !== view.h;
    const scrolled = sx !== view.sx || sy !== view.sy;
    Object.assign(view, { w: w | 0, h: h | 0, sx: sx | 0, sy: sy | 0, pw: pw | 0, ph: ph | 0, z: z > 0 ? z : 100 });
    if (resized) {
      global.innerWidth = global.outerWidth = global.screen.availWidth = global.screen.width = view.w;
      global.innerHeight = global.outerHeight = global.screen.availHeight = global.screen.height = view.h;
      dispatch(global, new Event('resize'));
      for (const q of queries) q._check();
    }
    if (scrolled) {
      dispatch(document, new Event('scroll', { bubbles: true }));
      dispatch(global, new Event('scroll'));
    }
    if (resized || scrolled) for (const f of listeners) { try { f(); } catch (e) { D.reportError(e); } }
  }
  for (const [k, get] of Object.entries({
    scrollX: () => view.sx, scrollY: () => view.sy, pageXOffset: () => view.sx, pageYOffset: () => view.sy,
    devicePixelRatio: () => view.z / 100,
  })) Object.defineProperty(global, k, { configurable: true, get, set() {} });
  define('visualViewport', Object.assign(new global.EventTarget(), {
    get width() { return view.w; }, get height() { return view.h; }, scale: 1, offsetLeft: 0, offsetTop: 0,
    get pageLeft() { return view.sx; }, get pageTop() { return view.sy; }, onresize: null, onscroll: null,
  }));

  // The box of `el` in page coordinates: its own and its descendants' (inline content is in runs).
  function pageRect(el) {
    if (!el || el.nodeType !== 1) return null;
    const hit = memo.get(el);
    if (hit !== undefined) return hit;
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    const own = boxes.get(el.uid);
    if (own && disp(own) === 0) { memo.set(el, null); return null; }
    if (own && flags(own) & F_BOX) { x0 = own[0]; y0 = own[1]; x1 = own[0] + own[2]; y1 = own[1] + own[3]; }
    if (!own || !(flags(own) & F_BOX) || disp(own) === 2) {
      for (const k of el.childNodes) {
        if (k.nodeType !== 1) continue;
        const r = pageRect(k);
        if (!r) continue;
        if (r[0] < x0) x0 = r[0];
        if (r[1] < y0) y0 = r[1];
        if (r[0] + r[2] > x1) x1 = r[0] + r[2];
        if (r[1] + r[3] > y1) y1 = r[1] + r[3];
      }
    }
    const r = x0 === Infinity ? null : [x0, y0, x1 - x0, y1 - y0];
    memo.set(el, r);
    return r;
  }
  const isRoot = (el) => el === document.documentElement;
  const isScroller = (el) => el === document.documentElement || el === document.body || el === document.scrollingElement;
  function rootRect() { return [0, 0, Math.max(view.w, view.pw), Math.max(view.h, view.ph)]; }
  function rectOf(el) { return isRoot(el) ? rootRect() : pageRect(el); }
  const styleRec = (el) => boxes.get(el.uid) || null;
  const pad = (rec, i) => (i === 0 ? rec[9] >> 16 : i === 1 ? (rec[9] << 16) >> 16 : i === 2 ? rec[10] >> 16 : (rec[10] << 16) >> 16);
  const mar = (rec, i) => (i === 0 ? rec[7] >> 16 : i === 1 ? (rec[7] << 16) >> 16 : i === 2 ? rec[8] >> 16 : (rec[8] << 16) >> 16);
  const bor = (rec, i) => (rec[11] >>> (i * 8)) & 255;

  kjs.layout = {
    rectOf, view, get generation() { return generation; },
    onUpdate(f) { listeners.push(f); },
    clientRect(el) { const r = rectOf(el); return r ? new DOMRect(r[0] - view.sx, r[1] - view.sy, r[2], r[3]) : new DOMRect(0, 0, 0, 0); },
  };

  // ---- element measures ----

  const P = Element.prototype;
  const get = (name, fn) => Object.defineProperty(P, name, { configurable: true, get: fn });
  const both = (name, g, s) => Object.defineProperty(P, name, { configurable: true, get: g, set: s });
  P.getBoundingClientRect = function () { return kjs.layout.clientRect(this); };
  P.getClientRects = function () { const r = rectOf(this); return r ? [kjs.layout.clientRect(this)] : []; };
  function offsetParentOf(el) {
    for (let p = el.parentElement; p; p = p.parentElement) {
      if (p === document.body) return p;
      const rec = styleRec(p);
      if (rec && flags(rec) & (F_ABS | F_FIXED | F_REL | F_STICKY)) return p;
      if (/^(td|th|table)$/.test(p.localName)) return p;
    }
    return null;
  }
  get('offsetParent', function () { return this.isConnected && rectOf(this) ? offsetParentOf(this) : null; });
  get('offsetWidth', function () { const r = rectOf(this); return r ? Math.round(r[2]) : 0; });
  get('offsetHeight', function () { const r = rectOf(this); return r ? Math.round(r[3]) : 0; });
  get('offsetLeft', function () {
    const r = rectOf(this); if (!r) return 0;
    const p = offsetParentOf(this); const pr = p && p !== document.body ? rectOf(p) : null;
    return Math.round(r[0] - (pr ? pr[0] : 0));
  });
  get('offsetTop', function () {
    const r = rectOf(this); if (!r) return 0;
    const p = offsetParentOf(this); const pr = p && p !== document.body ? rectOf(p) : null;
    return Math.round(r[1] - (pr ? pr[1] : 0));
  });
  get('clientWidth', function () {
    if (isScroller(this) && this !== document.body) return view.w;
    const r = rectOf(this); if (!r) return 0;
    const rec = styleRec(this);
    if (rec && disp(rec) === 2) return 0;
    return Math.max(0, Math.round(r[2] - (rec ? bor(rec, 1) + bor(rec, 3) : 0)));
  });
  get('clientHeight', function () {
    if (isScroller(this) && this !== document.body) return view.h;
    const r = rectOf(this); if (!r) return 0;
    const rec = styleRec(this);
    if (rec && disp(rec) === 2) return 0;
    return Math.max(0, Math.round(r[3] - (rec ? bor(rec, 0) + bor(rec, 2) : 0)));
  });
  get('clientTop', function () { const rec = styleRec(this); return rec ? bor(rec, 0) : 0; });
  get('clientLeft', function () { const rec = styleRec(this); return rec ? bor(rec, 3) : 0; });
  get('scrollWidth', function () { if (isScroller(this)) return Math.max(view.w, view.pw); const r = rectOf(this); return r ? Math.round(r[2]) : 0; });
  get('scrollHeight', function () { if (isScroller(this)) return Math.max(view.h, view.ph); const r = rectOf(this); return r ? Math.round(r[3]) : 0; });
  both('scrollTop', function () { return isScroller(this) ? view.sy : 0; }, function (v) { if (isScroller(this)) scrollTo(view.sx, v); });
  both('scrollLeft', function () { return isScroller(this) ? view.sx : 0; }, function (v) { if (isScroller(this)) scrollTo(v, view.sy); });
  P.scrollTo = function (x, y) { if (isScroller(this)) windowScroll(x, y, false); };
  P.scroll = P.scrollTo;
  P.scrollBy = function (x, y) { if (isScroller(this)) windowScroll(x, y, true); };
  P.scrollIntoView = function (opt) {
    const r = rectOf(this);
    if (!r) return;
    const block = typeof opt === 'object' && opt ? opt.block || 'start' : opt === false ? 'end' : 'start';
    let y = r[1];
    if (block === 'center') y = r[1] + r[3] / 2 - view.h / 2;
    else if (block === 'end') y = r[1] + r[3] - view.h;
    else if (block === 'nearest') {
      if (r[1] >= view.sy && r[1] + r[3] <= view.sy + view.h) return;
      y = r[1] < view.sy ? r[1] : r[1] + r[3] - view.h;
    }
    scrollTo(view.sx, y);
  };
  P.scrollIntoViewIfNeeded = function () { this.scrollIntoView({ block: 'nearest' }); };
  P.checkVisibility = function () { return !!rectOf(this) && getComputedStyle(this).visibility !== 'hidden'; };

  function scrollTo(x, y) {
    const maxY = Math.max(0, view.ph - view.h);
    x = Math.round(Number(x) || 0);
    y = Math.max(0, Math.min(maxY || Infinity, Math.round(Number(y) || 0)));
    if (y === view.sy && x === view.sx) return;
    send('S' + x + ' ' + y);
    // At once, as in a browser (the browser confirms with a `view` message).
    setView(view.w, view.h, 0, y, view.pw, view.ph, view.z);
  }
  function windowScroll(a, b, rel) {
    let x = a, y = b;
    if (a && typeof a === 'object') { x = a.left ?? (rel ? 0 : view.sx); y = a.top ?? (rel ? 0 : view.sy); }
    if (rel) { x = view.sx + (Number(x) || 0); y = view.sy + (Number(y) || 0); }
    scrollTo(x ?? view.sx, y ?? view.sy);
  }
  define('scrollTo', (a, b) => windowScroll(a, b, false));
  define('scroll', (a, b) => windowScroll(a, b, false));
  define('scrollBy', (a, b) => windowScroll(a, b, true));

  // ---- what is at a point ----

  function elementsFromPoint(x, y) {
    const px = x + view.sx, py = y + view.sy;
    const hits = [];
    for (const [uid, r] of boxes) {
      if (!(flags(r) & F_BOX)) continue;
      if (px >= r[0] && px < r[0] + r[2] && py >= r[1] && py < r[1] + r[3]) {
        const el = byUid.get(uid);
        if (el && el.isConnected) hits.push([r[2] * r[3], el]);
      }
    }
    hits.sort((a, b) => a[0] - b[0]);
    const out = hits.map((h) => h[1]);
    if (document.body && !out.includes(document.body)) out.push(document.body);
    if (document.documentElement && !out.includes(document.documentElement)) out.push(document.documentElement);
    return out;
  }
  D.Document.prototype.elementFromPoint = function (x, y) {
    if (x < 0 || y < 0 || x > view.w || y > view.h) return null;
    return elementsFromPoint(+x, +y)[0] || null;
  };
  D.Document.prototype.elementsFromPoint = function (x, y) { return elementsFromPoint(+x, +y); };
  D.Document.prototype.caretPositionFromPoint = function () { return null; };
  D.Document.prototype.caretRangeFromPoint = function () { return null; };

  // ---- computed style ----

  const INLINE = new Set(['a', 'abbr', 'b', 'bdi', 'bdo', 'br', 'cite', 'code', 'data', 'dfn', 'em', 'i',
    'kbd', 'label', 'mark', 'q', 's', 'samp', 'small', 'span', 'strong', 'sub', 'sup', 'time', 'u', 'var',
    'img', 'input', 'button', 'select', 'textarea', 'svg', 'canvas', 'video', 'audio', 'iframe', 'font', 'tt', 'big', 'slot']);
  const NONE = new Set(['head', 'script', 'style', 'title', 'meta', 'link', 'template', 'base', 'noscript']);
  const hex = (c) => 'rgb(' + ((c >> 16) & 255) + ', ' + ((c >> 8) & 255) + ', ' + (c & 255) + ')';
  const rgba = (c) => (c >>> 24) === 0 ? 'rgba(0, 0, 0, 0)' : (c >>> 24) === 255 ? hex(c) : 'rgba(' + ((c >> 16) & 255) + ', ' + ((c >> 8) & 255) + ', ' + (c & 255) + ', ' + Math.round((c >>> 24) / 2.55) / 100 + ')';
  const px = (n) => n + 'px';
  const SIDES = ['top', 'right', 'bottom', 'left'];

  function hiddenAbove(el) {
    for (let p = el; p && p.nodeType === 1; p = p.parentNode) {
      if (p.hasAttribute('hidden')) return true;
      const rec = styleRec(p);
      if (rec && disp(rec) === 0) return true;
      if (NONE.has(p.localName)) return true;
    }
    return false;
  }
  // The computed value of `name` (CSS name, lower case) for `el`, from the browser's snapshot and the
  // element's own `style` attribute.
  function computed(el, name) {
    const own = D.getDecl(el, name);
    if (own && !/^(inherit|initial|unset|revert)$/.test(own)) return own.replace(/\s*!important\s*$/, '');
    const rec = styleRec(el);
    const r = rectOf(el);
    switch (name) {
      case 'display':
        if (!el.isConnected || hiddenAbove(el)) return 'none';
        if (rec) return DISPLAY[disp(rec)] || 'block';
        return INLINE.has(el.localName) ? 'inline' : el.localName === 'li' ? 'list-item' : 'block';
      case 'visibility': return rec && flags(rec) & F_HIDDEN ? 'hidden' : 'visible';
      case 'opacity': return '1';
      case 'position': { const f = rec ? flags(rec) : 0; return f & F_FIXED ? 'fixed' : f & F_ABS ? 'absolute' : f & F_STICKY ? 'sticky' : f & F_REL ? 'relative' : 'static'; }
      case 'float': { const f = rec ? flags(rec) : 0; return f & F_FLOAT_L ? 'left' : f & F_FLOAT_R ? 'right' : 'none'; }
      case 'color': return rec ? hex(rec[4]) : el.parentElement ? computed(el.parentElement, 'color') : 'rgb(0, 0, 0)';
      case 'background-color': return rec ? rgba(rec[5]) : 'rgba(0, 0, 0, 0)';
      case 'font-size': return rec ? px(rec[6] >>> 16) : el.parentElement ? computed(el.parentElement, 'font-size') : '16px';
      case 'font-weight': return rec && flags(rec) & F_BOLD ? '700' : '400';
      case 'font-style': return rec && flags(rec) & F_ITALIC ? 'italic' : 'normal';
      case 'font-family': return rec && flags(rec) & F_MONO ? 'monospace' : 'sans-serif';
      case 'line-height': return 'normal';
      case 'width': {
        if (!r) return 'auto';
        const b = rec ? bor(rec, 1) + bor(rec, 3) + pad(rec, 1) + pad(rec, 3) : 0;
        return px(Math.max(0, r[2] - b));
      }
      case 'height': {
        if (!r) return 'auto';
        const b = rec ? bor(rec, 0) + bor(rec, 2) + pad(rec, 0) + pad(rec, 2) : 0;
        return px(Math.max(0, r[3] - b));
      }
      case 'box-sizing': return 'content-box';
      case 'overflow': case 'overflow-x': case 'overflow-y': return 'visible';
      case 'z-index': case 'top': case 'left': case 'right': case 'bottom': return 'auto';
      case 'transform': case 'animation-name': case 'transition-property': case 'max-width': case 'max-height': case 'filter': case 'clip-path': return 'none';
      case 'transition-duration': case 'animation-duration': case 'transition-delay': case 'animation-delay': return '0s';
      case 'text-align': return 'start';
      case 'direction': return el.closest && el.closest('[dir=rtl]') ? 'rtl' : 'ltr';
      case 'pointer-events': return 'auto';
      case 'cursor': return 'auto';
      case 'white-space': return 'normal';
      case 'content': return 'normal';
      case 'writing-mode': return 'horizontal-tb';
      case 'flex-direction': return 'row';
      case 'min-width': case 'min-height': return '0px';
    }
    let m = /^(margin|padding)-(top|right|bottom|left)$/.exec(name);
    if (m) return rec ? px(m[1] === 'margin' ? mar(rec, SIDES.indexOf(m[2])) : pad(rec, SIDES.indexOf(m[2]))) : '0px';
    m = /^border-(top|right|bottom|left)-width$/.exec(name);
    if (m) return rec ? px(bor(rec, SIDES.indexOf(m[1]))) : '0px';
    m = /^border-(top|right|bottom|left)-color$/.exec(name);
    if (m) return computed(el, 'color');
    m = /^border-(top|right|bottom|left)-style$/.exec(name);
    if (m) return rec && bor(rec, SIDES.indexOf(m[1])) ? 'solid' : 'none';
    if (name === 'margin' || name === 'padding') return SIDES.map((s) => computed(el, name + '-' + s)).join(' ');
    if (name === 'border-width') return SIDES.map((s) => computed(el, 'border-' + s + '-width')).join(' ');
    if (name.startsWith('--')) {
      for (let p = el; p && p.nodeType === 1; p = p.parentNode) { const v = D.getDecl(p, name); if (v) return v; }
      return '';
    }
    return '';
  }
  const COMMON = ['display', 'position', 'visibility', 'color', 'background-color', 'font-size', 'font-weight',
    'font-style', 'font-family', 'width', 'height', 'margin-top', 'margin-right', 'margin-bottom', 'margin-left',
    'padding-top', 'padding-right', 'padding-bottom', 'padding-left', 'opacity', 'float', 'overflow', 'z-index', 'transform'];
  function getComputedStyle(el, pseudo) {
    if (!el || el.nodeType !== 1) throw new TypeError("getComputedStyle: parameter 1 is not of type 'Element'");
    const pseudoBox = pseudo && /^::?(before|after|marker|placeholder)$/.test(String(pseudo));
    const value = (n) => (pseudoBox ? (n === 'content' ? 'none' : n === 'display' ? 'none' : '') : computed(el, n));
    return new Proxy({}, {
      get(_, p) {
        if (typeof p !== 'string') return undefined;
        switch (p) {
          case 'getPropertyValue': return (n) => value(String(n).trim().toLowerCase());
          case 'getPropertyPriority': return () => '';
          case 'item': return (i) => COMMON[i] || '';
          case 'length': return COMMON.length;
          case 'cssText': return COMMON.map((n) => n + ': ' + value(n) + ';').join(' ');
          case 'parentRule': return null;
          case 'setProperty': case 'removeProperty': return () => { throw new D.DOMException('read-only', 'NoModificationAllowedError'); };
          case 'cssFloat': return value('float');
        }
        if (/^\d+$/.test(p)) return COMMON[+p] || '';
        return value(D.kebab(p));
      },
      set() { return true; },
      has(_, p) { return typeof p === 'string'; },
    });
  }
  define('getComputedStyle', getComputedStyle);

  // ---- media queries ----

  // Does `q` (a media query list) hold now?
  function mediaMatches(q) {
    const one = (part) => {
      part = part.trim().toLowerCase();
      if (!part) return true;
      let not = false;
      if (part.startsWith('not ')) { not = true; part = part.slice(4); }
      part = part.replace(/^only\s+/, '');
      let ok = true;
      for (const term of part.split(/\s+and\s+/)) {
        const t = term.trim();
        if (t === 'all' || t === 'screen') continue;
        if (t === 'print' || t === 'speech' || t === 'tv' || t === 'handheld') { ok = false; continue; }
        const m = /^\(\s*([-a-z]+)\s*(?::\s*([^)]+))?\)$/.exec(t);
        if (!m) { ok = false; continue; }
        if (!feature(m[1], (m[2] || '').trim())) ok = false;
      }
      return not ? !ok : ok;
    };
    return String(q).split(',').some(one);
  }
  const len = (v) => {
    const m = /^(-?[\d.]+)\s*(px|em|rem|vw|vh)?$/.exec(v);
    if (!m) return NaN;
    const n = parseFloat(m[1]);
    return m[2] === 'em' || m[2] === 'rem' ? n * 16 : m[2] === 'vw' ? n * view.w / 100 : m[2] === 'vh' ? n * view.h / 100 : n;
  };
  function feature(name, v) {
    const dpr = view.z / 100;
    switch (name) {
      case 'min-width': return view.w >= len(v);
      case 'max-width': return view.w <= len(v);
      case 'width': return v ? view.w === len(v) : true;
      case 'min-height': return view.h >= len(v);
      case 'max-height': return view.h <= len(v);
      case 'height': return v ? view.h === len(v) : true;
      case 'orientation': return v === (view.h >= view.w ? 'portrait' : 'landscape');
      case 'prefers-color-scheme': return v === 'light';
      case 'prefers-reduced-motion': return v === 'no-preference';
      case 'prefers-contrast': return v === 'no-preference';
      case 'prefers-reduced-transparency': case 'prefers-reduced-data': return v === 'no-preference';
      case 'forced-colors': return v === 'none';
      case 'inverted-colors': return v === 'none';
      case 'hover': case 'any-hover': return !v || v === 'hover';
      case 'pointer': case 'any-pointer': return !v || v === 'fine';
      case 'color': return true;
      case 'monochrome': return v === '0';
      case 'display-mode': return v === 'browser';
      case 'scripting': return v === 'enabled';
      case 'update': return v === 'fast';
      case '-webkit-min-device-pixel-ratio': case 'min-resolution': return dpr >= parseFloat(v);
      case '-webkit-max-device-pixel-ratio': case 'max-resolution': return dpr <= parseFloat(v);
      case 'aspect-ratio': case 'min-aspect-ratio': case 'max-aspect-ratio': {
        const m = /^(\d+)\s*\/\s*(\d+)$/.exec(v);
        if (!m) return false;
        const want = +m[1] / +m[2], have = view.w / Math.max(1, view.h);
        return name === 'aspect-ratio' ? Math.abs(want - have) < 0.01 : name === 'min-aspect-ratio' ? have >= want : have <= want;
      }
      default: return false;
    }
  }
  const queries = new Set();
  class MediaQueryList extends global.EventTarget {
    constructor(q) { super(); this.media = String(q); this._m = mediaMatches(this.media); this.onchange = null; }
    get matches() { return mediaMatches(this.media); }
    addListener(f) { if (f) { this.addEventListener('change', f); queries.add(this); } }
    removeListener(f) { this.removeEventListener('change', f); }
    _check() {
      const m = mediaMatches(this.media);
      if (m === this._m) return;
      this._m = m;
      const ev = new Event('change');
      ev.matches = m; ev.media = this.media;
      dispatch(this, ev);
      if (typeof this.onchange === 'function') { try { this.onchange(ev); } catch (e) { D.reportError(e); } }
    }
  }
  const addMQ = MediaQueryList.prototype.addEventListener;
  MediaQueryList.prototype.addEventListener = function (t, f, o) { addMQ.call(this, t, f, o); queries.add(this); };
  define('MediaQueryList', MediaQueryList);
  define('matchMedia', (q) => { const l = new MediaQueryList(q); return l; });
  kjs.mediaMatches = mediaMatches;
  define('CSS', {
    supports(a, b) {
      const decl = b === undefined ? String(a) : a + ':' + b;
      return /^\(?\s*(display\s*:\s*(flex|grid|block|inline|none|inline-block)|color|background|margin|padding|width|height|position\s*:\s*(relative|absolute|static|sticky))/.test(decl.trim());
    },
    escape(s) { return String(s).replace(/([^\w-])/g, '\\$1').replace(/^(\d)/, '\\3$1 '); },
  });
})(globalThis, globalThis.__kjs_native, globalThis.__kjs);
