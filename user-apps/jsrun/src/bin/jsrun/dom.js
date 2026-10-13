// Kitsune jsrun --page: the DOM, events, timers and window of a browser page, in JavaScript.
//
// Evaluated once before the page's scripts. The browser sends the parsed document and the scripts
// (`init`) and then events (`click`); this code builds the document, runs the scripts as tasks, fires
// DOMContentLoaded and load, runs timers and animation frames, and after each batch of tasks sends the
// whole document back as HTML when something changed ("D" + html, every element with its id in
// data-kjs). See kitsune_core::web::script for the messages and page.c for the loop around this.
//
// The loop (page.c) calls __kjs.onMessage, runOne, nextDelay, flush and interrupted; it removes the
// two globals right after this file runs, so pages cannot reach them.
(function (global, native) {
  'use strict';
  const send = native.send;
  const evalScript = native.evalScript;
  const now = () => performance.now();

  let dirty = false;
  let nextUid = 1;
  const byUid = new Map();
  const touch = () => { dirty = true; };
  // Hooks for the files evaluated after this one (events.js, observers.js, structure.js...):
  // `mutators` see every change of the tree (MutationObserver, custom elements), `wire` decides what
  // the browser's copy of an element holds (live form values, shadow trees).
  const mutators = [];
  const notify = (type, target, a, b, c, d) => { for (const f of mutators) f(type, target, a, b, c, d); };
  const hooks = { wireAttrs: null, wireKids: null, created: null };

  const VOID = new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta',
    'source', 'track', 'wbr']);
  const RAW = new Set(['script', 'style']);
  const RCDATA = new Set(['textarea', 'title']);
  const NAME_OK = /^[a-zA-Z_:][-a-zA-Z0-9_:]*$/;

  function reportError(e) {
    const stack = e && e.stack ? '\n' + e.stack : '';
    console.error('Uncaught ' + String(e) + stack);
  }

  class DOMException extends Error {
    constructor(message, name) { super(message); this.name = name || 'Error'; }
  }

  // ---- events ----

  class Event {
    constructor(type, init) {
      init = init || {};
      this.type = String(type);
      this.bubbles = !!init.bubbles;
      this.cancelable = !!init.cancelable;
      this.composed = !!init.composed;
      this.defaultPrevented = false;
      this.target = null;
      this.currentTarget = null;
      this.eventPhase = 0;
      this.timeStamp = now();
      this.isTrusted = false;
      this._stop = false;
      this._stopNow = false;
      this._path = [];
    }
    preventDefault() { if (this.cancelable) this.defaultPrevented = true; }
    stopPropagation() { this._stop = true; }
    stopImmediatePropagation() { this._stop = true; this._stopNow = true; }
    get srcElement() { return this.target; }
    get returnValue() { return !this.defaultPrevented; }
    set returnValue(v) { if (v === false) this.preventDefault(); }
    get cancelBubble() { return this._stop; }
    set cancelBubble(v) { if (v) this._stop = true; }
    composedPath() { return this._path.slice(); }
    initEvent(type, bubbles, cancelable) {
      this.type = String(type); this.bubbles = !!bubbles; this.cancelable = !!cancelable;
    }
  }
  Event.NONE = 0; Event.CAPTURING_PHASE = 1; Event.AT_TARGET = 2; Event.BUBBLING_PHASE = 3;
  class CustomEvent extends Event {
    constructor(type, init) { super(type, init); this.detail = init && init.detail !== undefined ? init.detail : null; }
  }
  class UIEvent extends Event {
    constructor(type, init) { super(type, init); this.detail = (init && init.detail) || 0; this.view = global; }
  }
  class MouseEvent extends UIEvent {
    constructor(type, init) {
      super(type, init);
      init = init || {};
      this.clientX = this.pageX = this.x = this.offsetX = this.screenX = init.clientX || 0;
      this.clientY = this.pageY = this.y = this.offsetY = this.screenY = init.clientY || 0;
      this.button = init.button || 0;
      this.buttons = init.buttons || 0;
      this.ctrlKey = !!init.ctrlKey; this.shiftKey = !!init.shiftKey;
      this.altKey = !!init.altKey; this.metaKey = !!init.metaKey;
      this.relatedTarget = init.relatedTarget || null;
    }
  }
  class KeyboardEvent extends UIEvent {
    constructor(type, init) {
      super(type, init);
      init = init || {};
      this.key = init.key || ''; this.code = init.code || '';
      this.keyCode = this.which = init.keyCode || 0;
      this.ctrlKey = !!init.ctrlKey; this.shiftKey = !!init.shiftKey;
      this.altKey = !!init.altKey; this.metaKey = !!init.metaKey; this.repeat = !!init.repeat;
    }
  }
  class FocusEvent extends UIEvent {}
  class InputEvent extends UIEvent {}

  const HANDLERS = ['click', 'dblclick', 'mousedown', 'mouseup', 'mousemove', 'mouseover', 'mouseout',
    'mouseenter', 'mouseleave', 'keydown', 'keyup', 'keypress', 'input', 'change', 'submit', 'reset',
    'focus', 'blur', 'load', 'error', 'resize', 'scroll', 'readystatechange', 'hashchange',
    'popstate', 'contextmenu', 'wheel', 'pointerdown', 'pointerup', 'touchstart', 'touchend'];

  function captureOf(opts) { return typeof opts === 'boolean' ? opts : !!(opts && opts.capture); }

  const targetMethods = {
    addEventListener(type, fn, opts) {
      if (!fn) return;
      const capture = captureOf(opts);
      const once = !!(opts && typeof opts === 'object' && opts.once);
      if (!this._ls) this._ls = Object.create(null);
      const list = this._ls[type] || (this._ls[type] = []);
      if (list.some((l) => l.fn === fn && l.capture === capture)) return;
      list.push({ fn, capture, once });
    },
    removeEventListener(type, fn, opts) {
      const list = this._ls && this._ls[type];
      if (!list) return;
      const capture = captureOf(opts);
      const i = list.findIndex((l) => l.fn === fn && l.capture === capture);
      if (i >= 0) list.splice(i, 1);
    },
    dispatchEvent(ev) { return dispatch(this, ev); },
  };

  // Handler properties (`el.onclick = f`) and attributes (`<button onclick="...">`).
  function handlerOf(node, type) {
    const own = node._on && node._on[type];
    if (own !== undefined) return own;
    if (node.nodeType !== 1) return null;
    const src = node.getAttribute('on' + type);
    if (src === null) return null;
    const cache = node._attrFn || (node._attrFn = Object.create(null));
    const c = cache[type];
    if (c && c.src === src) return c.fn;
    let fn = null;
    try {
      fn = new Function('event', src);
    } catch (e) {
      reportError(e);
    }
    cache[type] = { src, fn };
    return fn;
  }

  function callListener(node, fn, ev) {
    try {
      if (typeof fn === 'function') return fn.call(node, ev);
      if (fn && typeof fn.handleEvent === 'function') return fn.handleEvent(ev);
    } catch (e) {
      reportError(e);
    }
    return undefined;
  }

  function invoke(node, ev, phase) {
    ev.currentTarget = node;
    ev.eventPhase = phase;
    const list = node._ls && node._ls[ev.type];
    if (list) {
      for (const l of list.slice()) {
        if ((phase === 1 && !l.capture) || (phase === 3 && l.capture)) continue;
        if (l.once) node.removeEventListener(ev.type, l.fn, l.capture);
        callListener(node, l.fn, ev);
        if (ev._stopNow) return;
      }
    }
    if (phase !== 1) {
      const h = handlerOf(node, ev.type);
      if (h && callListener(node, h, ev) === false) ev.preventDefault();
    }
  }

  function dispatch(target, ev) {
    ev.target = target;
    const path = [];
    // A shadow root's host is next on the path (structure.js sets `_kjsHost`).
    for (let n = target; n; n = n.parentNode || n._kjsHost || null) path.push(n);
    if (path[path.length - 1] === document && ev.type !== 'load') path.push(global);
    ev._path = path;
    for (let i = path.length - 1; i > 0 && !ev._stop; i--) invoke(path[i], ev, 1);
    if (!ev._stop) invoke(path[0], ev, 2);
    if (ev.bubbles) for (let i = 1; i < path.length && !ev._stop; i++) invoke(path[i], ev, 3);
    ev.currentTarget = null;
    ev.eventPhase = 0;
    return !ev.defaultPrevented;
  }

  function addHandlerProps(proto) {
    for (const type of HANDLERS) {
      Object.defineProperty(proto, 'on' + type, {
        configurable: true,
        get() { return (this._on && this._on[type]) || null; },
        set(fn) {
          if (!this._on) this._on = Object.create(null);
          this._on[type] = typeof fn === 'function' ? fn : null;
        },
      });
    }
  }

  // ---- nodes ----

  class Node {
    constructor(type, name) {
      this.nodeType = type;
      this.nodeName = name;
      this.parentNode = null;
      this.childNodes = [];
      this._ls = null;
      this._on = null;
    }
    get ownerDocument() { return this === document ? null : document; }
    get firstChild() { return this.childNodes[0] || null; }
    get lastChild() { return this.childNodes[this.childNodes.length - 1] || null; }
    get parentElement() { const p = this.parentNode; return p && p.nodeType === 1 ? p : null; }
    get nextSibling() {
      const p = this.parentNode;
      return p ? p.childNodes[p.childNodes.indexOf(this) + 1] || null : null;
    }
    get previousSibling() {
      const p = this.parentNode;
      return p ? p.childNodes[p.childNodes.indexOf(this) - 1] || null : null;
    }
    get isConnected() { let n = this; while (n.parentNode) n = n.parentNode; return n === document; }
    get nodeValue() { return null; }
    set nodeValue(v) {}
    getRootNode() { let n = this; while (n.parentNode) n = n.parentNode; return n; }
    hasChildNodes() { return this.childNodes.length > 0; }
    contains(n) { for (; n; n = n.parentNode) if (n === this) return true; return false; }
    appendChild(c) { return this.insertBefore(c, null); }
    insertBefore(c, ref) {
      if (!(c instanceof Node)) throw new TypeError('insertBefore: not a Node');
      if (c.nodeType === 11) {
        for (const k of c.childNodes.slice()) this.insertBefore(k, ref);
        return c;
      }
      if (c.contains(this)) throw new DOMException('the new child contains the parent', 'HierarchyRequestError');
      if (ref === c) return c;
      if (c.parentNode) c.parentNode._detach(c);
      let i = -1;
      if (ref) {
        i = this.childNodes.indexOf(ref);
        if (i < 0) throw new DOMException('the reference is not a child', 'NotFoundError');
      }
      if (i < 0) this.childNodes.push(c); else this.childNodes.splice(i, 0, c);
      c.parentNode = this;
      touch();
      if (mutators.length) notify('childList', this, [c], [], c.previousSibling, c.nextSibling);
      return c;
    }
    removeChild(c) {
      if (!c || c.parentNode !== this) throw new DOMException('not a child', 'NotFoundError');
      this._detach(c);
      return c;
    }
    replaceChild(n, old) { this.insertBefore(n, old); if (n !== old) this.removeChild(old); return old; }
    _detach(c) {
      const i = this.childNodes.indexOf(c);
      const prev = i > 0 ? this.childNodes[i - 1] : null;
      const next = i >= 0 ? this.childNodes[i + 1] || null : null;
      if (i >= 0) this.childNodes.splice(i, 1);
      c.parentNode = null;
      touch();
      if (mutators.length) notify('childList', this, [], [c], prev, next);
    }
    _dropAll() {
      const old = this.childNodes;
      for (const k of old) k.parentNode = null;
      this.childNodes = [];
      if (old.length && mutators.length) notify('childList', this, [], old, null, null);
    }
    get textContent() {
      let out = '';
      const walk = (n) => {
        for (const k of n.childNodes) {
          if (k.nodeType === 3) out += k._data;
          else if (k.nodeType === 1) walk(k);
        }
      };
      walk(this);
      return out;
    }
    set textContent(v) {
      this._dropAll();
      if (v !== null && v !== undefined && v !== '') this.appendChild(new Text(String(v)));
      touch();
    }
    cloneNode(deep) {
      const c = this._shallow();
      if (deep) for (const k of this.childNodes) c.appendChild(k.cloneNode(true));
      return c;
    }
    isEqualNode(o) { return !!o && serialize(this, false) === serialize(o, false); }
    isSameNode(o) { return o === this; }
    compareDocumentPosition(o) { return this.contains(o) ? 20 : o.contains(this) ? 10 : 4; }
    normalize() {}
  }
  Object.assign(Node.prototype, targetMethods);
  Object.assign(Node, { ELEMENT_NODE: 1, TEXT_NODE: 3, COMMENT_NODE: 8, DOCUMENT_NODE: 9,
    DOCUMENT_FRAGMENT_NODE: 11 });
  addHandlerProps(Node.prototype);

  function toNodes(args) {
    return args.map((a) => (a instanceof Node ? a : new Text(String(a))));
  }
  const parentMethods = {
    append(...a) { for (const n of toNodes(a)) this.appendChild(n); },
    prepend(...a) { const f = this.firstChild; for (const n of toNodes(a)) this.insertBefore(n, f); },
    replaceChildren(...a) { this.textContent = ''; this.append(...a); },
    get children() { return this.childNodes.filter((n) => n.nodeType === 1); },
    get firstElementChild() { return this.childNodes.find((n) => n.nodeType === 1) || null; },
    get lastElementChild() {
      for (let i = this.childNodes.length - 1; i >= 0; i--) if (this.childNodes[i].nodeType === 1) return this.childNodes[i];
      return null;
    },
    get childElementCount() { return this.children.length; },
    querySelector(sel) { return query(this, sel, true)[0] || null; },
    querySelectorAll(sel) { return query(this, sel, false); },
    getElementsByTagName(tag) {
      tag = String(tag).toLowerCase();
      return descendants(this).filter((e) => tag === '*' || e.localName === tag);
    },
    getElementsByClassName(names) {
      const want = String(names).split(/\s+/).filter(Boolean);
      return descendants(this).filter((e) => want.every((c) => e.classList.contains(c)));
    },
  };
  const childMethods = {
    remove() { if (this.parentNode) this.parentNode._detach(this); },
    before(...a) { const p = this.parentNode; if (p) for (const n of toNodes(a)) p.insertBefore(n, this); },
    after(...a) {
      const p = this.parentNode;
      if (!p) return;
      const next = this.nextSibling;
      for (const n of toNodes(a)) p.insertBefore(n, next);
    },
    replaceWith(...a) { const p = this.parentNode; if (p) { this.before(...a); p._detach(this); } },
    get nextElementSibling() {
      for (let n = this.nextSibling; n; n = n.nextSibling) if (n.nodeType === 1) return n;
      return null;
    },
    get previousElementSibling() {
      for (let n = this.previousSibling; n; n = n.previousSibling) if (n.nodeType === 1) return n;
      return null;
    },
  };
  function mixin(target, src) {
    for (const k of Object.getOwnPropertyNames(src)) Object.defineProperty(target, k, Object.getOwnPropertyDescriptor(src, k));
  }

  function descendants(root) {
    const out = [];
    const walk = (n) => {
      for (const k of n.childNodes) if (k.nodeType === 1) { out.push(k); walk(k); }
    };
    walk(root);
    return out;
  }

  class CharacterData extends Node {
    constructor(type, name, data) { super(type, name); this._data = String(data); }
    get data() { return this._data; }
    set data(v) {
      const old = this._data;
      this._data = String(v);
      touch();
      if (mutators.length) notify('characterData', this, old);
    }
    get nodeValue() { return this._data; }
    set nodeValue(v) { this.data = v; }
    get textContent() { return this._data; }
    set textContent(v) { this.data = v; }
    get length() { return this._data.length; }
    appendData(s) { this.data = this._data + s; }
  }
  mixin(CharacterData.prototype, childMethods);
  class Text extends CharacterData {
    constructor(data) { super(3, '#text', data === undefined ? '' : data); }
    get wholeText() { return this._data; }
    _shallow() { return new Text(this._data); }
  }
  class Comment extends CharacterData {
    constructor(data) { super(8, '#comment', data === undefined ? '' : data); }
    _shallow() { return new Comment(this._data); }
  }
  class DocumentFragment extends Node {
    constructor() { super(11, '#document-fragment'); }
    _shallow() { return new DocumentFragment(); }
    getElementById(id) { return descendants(this).find((e) => e.id === String(id)) || null; }
  }
  mixin(DocumentFragment.prototype, parentMethods);

  // ---- elements ----

  function kebab(p) {
    if (p === 'cssFloat') return 'float';
    if (p.startsWith('--')) return p;
    return p.replace(/^(webkit|moz|ms)(?=[A-Z])/, '-$1').replace(/[A-Z]/g, (c) => '-' + c.toLowerCase());
  }
  function camel(p) { return p.replace(/-([a-z])/g, (_, c) => c.toUpperCase()); }

  function decls(el) {
    const out = [];
    for (const part of (el.getAttribute('style') || '').split(';')) {
      const i = part.indexOf(':');
      if (i > 0) out.push([part.slice(0, i).trim().toLowerCase(), part.slice(i + 1).trim()]);
    }
    return out;
  }
  function setDecl(el, name, value) {
    name = String(name).trim().toLowerCase();
    const list = decls(el).filter((d) => d[0] !== name);
    if (value !== null && value !== undefined && String(value) !== '') list.push([name, String(value)]);
    el.setAttribute('style', list.map((d) => d[0] + ': ' + d[1]).join('; '));
  }
  function getDecl(el, name) {
    const d = decls(el).find((x) => x[0] === String(name).toLowerCase());
    return d ? d[1] : '';
  }
  function styleOf(el) {
    return new Proxy({}, {
      get(_, prop) {
        if (prop === 'setProperty') return (n, v) => setDecl(el, n, v);
        if (prop === 'getPropertyValue') return (n) => getDecl(el, n);
        if (prop === 'removeProperty') return (n) => { const o = getDecl(el, n); setDecl(el, n, ''); return o; };
        if (prop === 'cssText') return el.getAttribute('style') || '';
        if (prop === 'length') return decls(el).length;
        if (typeof prop !== 'string') return undefined;
        return getDecl(el, kebab(prop));
      },
      set(_, prop, v) {
        if (prop === 'cssText') el.setAttribute('style', String(v));
        else if (typeof prop === 'string') setDecl(el, kebab(prop), v);
        return true;
      },
    });
  }

  class DOMTokenList {
    constructor(el, attr) { this._el = el; this._attr = attr; }
    _get() { return (this._el.getAttribute(this._attr) || '').split(/\s+/).filter(Boolean); }
    _set(list) { this._el.setAttribute(this._attr, list.join(' ')); }
    get length() { return this._get().length; }
    get value() { return this._el.getAttribute(this._attr) || ''; }
    set value(v) { this._el.setAttribute(this._attr, String(v)); }
    item(i) { return this._get()[i] || null; }
    contains(t) { return this._get().includes(String(t)); }
    add(...ts) { const l = this._get(); for (const t of ts) if (!l.includes(String(t))) l.push(String(t)); this._set(l); }
    remove(...ts) { const r = ts.map(String); this._set(this._get().filter((t) => !r.includes(t))); }
    toggle(t, force) {
      t = String(t);
      const has = this.contains(t);
      const want = force === undefined ? !has : !!force;
      if (want && !has) this.add(t);
      if (!want && has) this.remove(t);
      return want;
    }
    replace(a, b) {
      const l = this._get();
      const i = l.indexOf(String(a));
      if (i < 0) return false;
      l[i] = String(b);
      this._set(l);
      return true;
    }
    forEach(fn, self) { this._get().forEach(fn, self); }
    toString() { return this.value; }
    [Symbol.iterator]() { return this._get()[Symbol.iterator](); }
  }

  const ZERO_RECT = Object.freeze({ x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0 });

  class Element extends Node {
    constructor(tag, uid) {
      tag = String(tag).toLowerCase();
      super(1, tag.toUpperCase());
      this.localName = tag;
      this._attrs = new Map();
      this.uid = uid || nextUid++;
      if (this.uid >= nextUid) nextUid = this.uid + 1;
      byUid.set(this.uid, this);
    }
    get tagName() { return this.nodeName; }
    _shallow() {
      const c = createElement(this.localName);
      for (const [k, v] of this._attrs) c._attrs.set(k, v);
      return c;
    }
    getAttribute(n) { const v = this._attrs.get(String(n).toLowerCase()); return v === undefined ? null : v; }
    setAttribute(n, v) {
      n = String(n).toLowerCase();
      const old = this._attrs.get(n);
      this._attrs.set(n, String(v));
      touch();
      if (mutators.length) notify('attributes', this, n, old === undefined ? null : old);
    }
    removeAttribute(n) {
      n = String(n).toLowerCase();
      const old = this._attrs.get(n);
      if (this._attrs.delete(n)) {
        touch();
        if (mutators.length) notify('attributes', this, n, old);
      }
    }
    hasAttribute(n) { return this._attrs.has(String(n).toLowerCase()); }
    hasAttributes() { return this._attrs.size > 0; }
    toggleAttribute(n, force) {
      const has = this.hasAttribute(n);
      const want = force === undefined ? !has : !!force;
      if (want && !has) this.setAttribute(n, '');
      if (!want && has) this.removeAttribute(n);
      return want;
    }
    getAttributeNames() { return [...this._attrs.keys()]; }
    get attributes() { return [...this._attrs].map(([name, value]) => ({ name, value, nodeName: name, nodeValue: value })); }
    getAttributeNS(_, n) { return this.getAttribute(n); }
    setAttributeNS(_, n, v) { this.setAttribute(n, v); }
    removeAttributeNS(_, n) { this.removeAttribute(n); }
    get id() { return this.getAttribute('id') || ''; }
    set id(v) { this.setAttribute('id', v); }
    get className() { return this.getAttribute('class') || ''; }
    set className(v) { this.setAttribute('class', v); }
    get classList() { return new DOMTokenList(this, 'class'); }
    get style() { return styleOf(this); }
    set style(v) { this.setAttribute('style', String(v)); }
    get dataset() {
      const el = this;
      return new Proxy({}, {
        get(_, p) { return typeof p === 'string' ? (el.getAttribute('data-' + kebab(p)) ?? undefined) : undefined; },
        set(_, p, v) { el.setAttribute('data-' + kebab(String(p)), v); return true; },
        has(_, p) { return el.hasAttribute('data-' + kebab(String(p))); },
        deleteProperty(_, p) { el.removeAttribute('data-' + kebab(String(p))); return true; },
        ownKeys() { return el.getAttributeNames().filter((n) => n.startsWith('data-')).map((n) => camel(n.slice(5))); },
        getOwnPropertyDescriptor(_, p) {
          const v = el.getAttribute('data-' + kebab(String(p)));
          return v === null ? undefined : { value: v, enumerable: true, configurable: true, writable: true };
        },
      });
    }
    get innerHTML() { return this.childNodes.map((n) => serialize(n, false)).join(''); }
    set innerHTML(html) {
      this._dropAll();
      for (const n of parseHTML(String(html))) this.appendChild(n);
      touch();
    }
    get outerHTML() { return serialize(this, false); }
    set outerHTML(html) {
      const p = this.parentNode;
      if (!p) return;
      for (const n of parseHTML(String(html))) p.insertBefore(n, this);
      p._detach(this);
    }
    insertAdjacentHTML(pos, html) { this._adjacent(pos, parseHTML(String(html))); }
    insertAdjacentElement(pos, el) { this._adjacent(pos, [el]); return el; }
    insertAdjacentText(pos, t) { this._adjacent(pos, [new Text(String(t))]); }
    _adjacent(pos, nodes) {
      switch (String(pos).toLowerCase()) {
        case 'beforebegin': if (this.parentNode) for (const n of nodes) this.parentNode.insertBefore(n, this); break;
        case 'afterbegin': { const f = this.firstChild; for (const n of nodes) this.insertBefore(n, f); break; }
        case 'beforeend': for (const n of nodes) this.appendChild(n); break;
        case 'afterend': if (this.parentNode) { const nx = this.nextSibling; for (const n of nodes) this.parentNode.insertBefore(n, nx); } break;
        default: throw new DOMException('bad position', 'SyntaxError');
      }
    }
    get innerText() { return this.textContent; }
    set innerText(v) { this.textContent = v; }
    get outerText() { return this.textContent; }
    matches(sel) { return parseSelector(sel).some((c) => matchComplex(this, c, c.length - 1)); }
    webkitMatchesSelector(sel) { return this.matches(sel); }
    closest(sel) {
      const list = parseSelector(sel);
      for (let e = this; e && e.nodeType === 1; e = e.parentNode) if (list.some((c) => matchComplex(e, c, c.length - 1))) return e;
      return null;
    }
    get hidden() { return this.hasAttribute('hidden'); }
    set hidden(v) { this.toggleAttribute('hidden', !!v); }
    get value() {
      if (this._kjsV !== undefined) return this._kjsV;
      if (this.localName === 'textarea') return this.textContent;
      if (this.localName === 'select') {
        const opts = this.getElementsByTagName('option');
        const o = opts.find((x) => x.hasAttribute('selected')) || opts[0];
        return o ? o.value : '';
      }
      if (this.localName === 'option') return this.getAttribute('value') ?? this.textContent;
      return this.getAttribute('value') || '';
    }
    set value(v) {
      v = String(v);
      this._kjsV = v;
      if (this.localName === 'textarea') this.textContent = v;
      else if (this.localName === 'select') {
        for (const o of this.getElementsByTagName('option')) o.toggleAttribute('selected', o.value === v);
      } else this.setAttribute('value', v);
    }
    get checked() { return this.hasAttribute('checked'); }
    set checked(v) { this.toggleAttribute('checked', !!v); }
    get disabled() { return this.hasAttribute('disabled'); }
    set disabled(v) { this.toggleAttribute('disabled', !!v); }
    get selected() { return this.hasAttribute('selected'); }
    set selected(v) { this.toggleAttribute('selected', !!v); }
    get form() { return this.closest('form'); }
    get options() { return this.getElementsByTagName('option'); }
    get selectedIndex() { return this.options.findIndex((o) => o.selected); }
    get text() { return this.textContent; }
    set text(v) { this.textContent = v; }
    click() { dispatch(this, new MouseEvent('click', { bubbles: true, cancelable: true })); }
    focus() { document._active = this; }
    blur() { if (document._active === this) document._active = null; }
    submit() {}
    reset() {}
    scrollIntoView() {}
    scrollTo() {}
    getBoundingClientRect() { return ZERO_RECT; }
    getClientRects() { return []; }
    get offsetWidth() { return 0; }
    get offsetHeight() { return 0; }
    get offsetTop() { return 0; }
    get offsetLeft() { return 0; }
    get offsetParent() { return this.parentElement; }
    get clientWidth() { return 0; }
    get clientHeight() { return 0; }
    get scrollWidth() { return 0; }
    get scrollHeight() { return 0; }
    get scrollTop() { return 0; }
    set scrollTop(v) {}
    get scrollLeft() { return 0; }
    set scrollLeft(v) {}
    get content() { const f = new DocumentFragment(); for (const k of this.childNodes) f.appendChild(k.cloneNode(true)); return f; }
    attachShadow() { return this; }
  }
  for (const a of ['href', 'src', 'type', 'name', 'title', 'alt', 'placeholder', 'rel', 'target',
    'action', 'method', 'lang', 'dir', 'role', 'width', 'height', 'min', 'max', 'step', 'pattern',
    'accept', 'autocomplete', 'download', 'colSpan', 'rowSpan', 'tabIndex', 'htmlFor']) {
    const attr = a === 'htmlFor' ? 'for' : a.toLowerCase();
    Object.defineProperty(Element.prototype, a, {
      configurable: true,
      get() { return this.getAttribute(attr) ?? ''; },
      set(v) { this.setAttribute(attr, v); },
    });
  }
  mixin(Element.prototype, parentMethods);
  mixin(Element.prototype, childMethods);
  class HTMLElement extends Element {}

  function createElement(tag, uid) {
    const el = new HTMLElement(tag, uid);
    return hooks.created ? hooks.created(el) : el;
  }

  // ---- selectors ----

  const selCache = new Map();
  function parseSelector(src) {
    src = String(src);
    const hit = selCache.get(src);
    if (hit) return hit;
    const s = src.trim();
    let i = 0;
    const bad = () => { throw new DOMException("'" + src + "' is not a valid selector", 'SyntaxError'); };
    const ident = () => {
      const m = /^-?(?:[a-zA-Z_ -￿]|\\.)(?:[-a-zA-Z0-9_ -￿]|\\.)*/.exec(s.slice(i));
      if (!m) bad();
      i += m[0].length;
      return m[0].replace(/\\(.)/g, '$1');
    };
    const groups = [];
    let complex = [];
    let comb = null;
    while (i <= s.length) {
      const c = s[i];
      if (i === s.length || c === ',') {
        if (!complex.length || comb) bad();
        groups.push(complex);
        complex = [];
        i++;
        if (i > s.length) break;
        continue;
      }
      if (/\s/.test(c)) { if (complex.length && !comb) comb = ' '; i++; continue; }
      if (c === '>' || c === '+' || c === '~') {
        if (!complex.length || (comb && comb !== ' ')) bad();
        comb = c; i++; continue;
      }
      const part = { tag: null, id: null, cls: [], attrs: [], pseudo: [] };
      let any = false;
      for (;;) {
        const d = s[i];
        if (d === '*') { i++; any = true; }
        else if (d === '#') { i++; part.id = ident(); any = true; }
        else if (d === '.') { i++; part.cls.push(ident()); any = true; }
        else if (d === '[') {
          const end = s.indexOf(']', i);
          if (end < 0) bad();
          const body = s.slice(i + 1, end).trim();
          i = end + 1;
          const m = /^([-\w:]+)\s*(?:([~|^$*]?=)\s*("([^"]*)"|'([^']*)'|[^\s\]]+)\s*(i)?)?$/.exec(body);
          if (!m) bad();
          const val = m[4] !== undefined ? m[4] : m[5] !== undefined ? m[5] : m[3];
          part.attrs.push({ name: m[1].toLowerCase(), op: m[2] || null, value: val, ci: !!m[6] });
          any = true;
        } else if (d === ':') {
          i++;
          if (s[i] === ':') { i++; ident(); part.pseudo.push({ name: 'never' }); any = true; continue; }
          const name = ident().toLowerCase();
          let arg = null;
          if (s[i] === '(') {
            let depth = 0;
            const start = i + 1;
            for (; i < s.length; i++) {
              if (s[i] === '(') depth++;
              else if (s[i] === ')' && --depth === 0) break;
            }
            if (i >= s.length) bad();
            arg = s.slice(start, i).trim();
            i++;
          }
          part.pseudo.push({ name, arg });
          any = true;
        } else if (d !== undefined && /[-a-zA-Z_\\ -￿]/.test(d) && !any) {
          part.tag = ident().toLowerCase();
          any = true;
        } else break;
      }
      if (!any) bad();
      complex.push({ comb: complex.length ? comb || ' ' : null, part });
      comb = null;
    }
    if (selCache.size > 200) selCache.clear();
    selCache.set(src, groups);
    return groups;
  }

  function nth(arg, pos) {
    const a = String(arg).replace(/\s+/g, '').toLowerCase();
    if (a === 'odd') return pos % 2 === 1;
    if (a === 'even') return pos % 2 === 0;
    const m = /^([+-]?\d*)n([+-]\d+)?$/.exec(a);
    if (!m) return pos === parseInt(a, 10);
    const k = m[1] === '' || m[1] === '+' ? 1 : m[1] === '-' ? -1 : parseInt(m[1], 10);
    const b = m[2] ? parseInt(m[2], 10) : 0;
    if (k === 0) return pos === b;
    const n = (pos - b) / k;
    return n >= 0 && Number.isInteger(n);
  }

  function siblingsOf(el) { return el.parentNode ? el.parentNode.childNodes.filter((n) => n.nodeType === 1) : [el]; }

  function matchPart(el, p) {
    if (p.tag && el.localName !== p.tag) return false;
    if (p.id !== null && el.getAttribute('id') !== p.id) return false;
    for (const c of p.cls) if (!el.classList.contains(c)) return false;
    for (const a of p.attrs) {
      let v = el.getAttribute(a.name);
      if (v === null) return false;
      if (!a.op) continue;
      let want = a.value;
      if (a.ci) { v = v.toLowerCase(); want = want.toLowerCase(); }
      switch (a.op) {
        case '=': if (v !== want) return false; break;
        case '~=': if (!v.split(/\s+/).includes(want)) return false; break;
        case '|=': if (v !== want && !v.startsWith(want + '-')) return false; break;
        case '^=': if (!want || !v.startsWith(want)) return false; break;
        case '$=': if (!want || !v.endsWith(want)) return false; break;
        case '*=': if (!want || !v.includes(want)) return false; break;
      }
    }
    for (const ps of p.pseudo) {
      const sib = () => siblingsOf(el);
      const typed = () => sib().filter((n) => n.localName === el.localName);
      switch (ps.name) {
        case 'first-child': if (sib()[0] !== el) return false; break;
        case 'last-child': { const s = sib(); if (s[s.length - 1] !== el) return false; break; }
        case 'only-child': if (sib().length !== 1) return false; break;
        case 'first-of-type': if (typed()[0] !== el) return false; break;
        case 'last-of-type': { const s = typed(); if (s[s.length - 1] !== el) return false; break; }
        case 'only-of-type': if (typed().length !== 1) return false; break;
        case 'nth-child': if (!nth(ps.arg, sib().indexOf(el) + 1)) return false; break;
        case 'nth-last-child': { const s = sib(); if (!nth(ps.arg, s.length - s.indexOf(el))) return false; break; }
        case 'nth-of-type': if (!nth(ps.arg, typed().indexOf(el) + 1)) return false; break;
        case 'nth-last-of-type': { const s = typed(); if (!nth(ps.arg, s.length - s.indexOf(el))) return false; break; }
        case 'not': if (el.matches(ps.arg)) return false; break;
        case 'is': case 'where': case 'matches': if (!el.matches(ps.arg)) return false; break;
        case 'has': if (!el.querySelector(ps.arg.replace(/^\s*>/, ':scope >'))) return false; break;
        case 'empty': if (el.childNodes.some((n) => n.nodeType === 1 || (n.nodeType === 3 && n._data))) return false; break;
        case 'root': if (el !== document.documentElement) return false; break;
        case 'checked': if (!el.hasAttribute('checked') && !el.hasAttribute('selected')) return false; break;
        case 'disabled': if (!el.hasAttribute('disabled')) return false; break;
        case 'enabled': if (el.hasAttribute('disabled')) return false; break;
        case 'link': case 'any-link': if (!(el.localName === 'a' && el.hasAttribute('href'))) return false; break;
        case 'scope': if (el !== scopeEl) return false; break;
        default: return false; // :hover, :focus, pseudo-elements: never in a script's view
      }
    }
    return true;
  }
  let scopeEl = null;

  function matchComplex(el, parts, idx) {
    const { comb, part } = parts[idx];
    if (!matchPart(el, part)) return false;
    if (idx === 0) return true;
    switch (comb) {
      case '>': { const p = el.parentElement; return !!p && matchComplex(p, parts, idx - 1); }
      case '+': { const s = el.previousElementSibling; return !!s && matchComplex(s, parts, idx - 1); }
      case '~':
        for (let s = el.previousElementSibling; s; s = s.previousElementSibling) if (matchComplex(s, parts, idx - 1)) return true;
        return false;
      default:
        for (let a = el.parentElement; a; a = a.parentElement) if (matchComplex(a, parts, idx - 1)) return true;
        return false;
    }
  }

  function query(root, sel, first) {
    const groups = parseSelector(sel);
    const out = [];
    const saved = scopeEl;
    scopeEl = root.nodeType === 1 ? root : null;
    try {
      const walk = (n) => {
        for (const k of n.childNodes) {
          if (k.nodeType !== 1) continue;
          if (groups.some((c) => matchComplex(k, c, c.length - 1))) {
            out.push(k);
            if (first) return true;
          }
          if (walk(k)) return true;
        }
        return false;
      };
      walk(root);
    } finally {
      scopeEl = saved;
    }
    return out;
  }

  // ---- HTML: parsing fragments and serializing ----

  const ENTITIES = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' ', copy: '©',
    reg: '®', trade: '™', hellip: '…', mdash: '—', ndash: '–', laquo: '«',
    raquo: '»', middot: '·', bull: '•', euro: '€', times: '×', larr: '←',
    rarr: '→', uarr: '↑', darr: '↓', deg: '°', ccedil: 'ç', atilde: 'ã',
    otilde: 'õ', aacute: 'á', eacute: 'é', iacute: 'í', oacute: 'ó',
    uacute: 'ú', ecirc: 'ê', ocirc: 'ô', acirc: 'â', agrave: 'à' };
  function decode(s) {
    if (s.indexOf('&') < 0) return s;
    return s.replace(/&(#x[0-9a-f]+|#\d+|[a-z]+\d*);?/gi, (m, e) => {
      if (e[0] === '#') {
        const cp = e[1] === 'x' || e[1] === 'X' ? parseInt(e.slice(2), 16) : parseInt(e.slice(1), 10);
        return cp > 0 && cp <= 0x10ffff ? String.fromCodePoint(cp) : '�';
      }
      const v = ENTITIES[e] ?? ENTITIES[e.toLowerCase()];
      return v === undefined ? m : v;
    });
  }

  // Close an open <p> (or list item, option, cell) when what starts cannot be inside it.
  const CLOSES_P = new Set(['p', 'div', 'ul', 'ol', 'dl', 'table', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6',
    'section', 'article', 'header', 'footer', 'nav', 'aside', 'main', 'form', 'pre', 'blockquote',
    'hr', 'figure', 'address', 'fieldset']);
  const SIBLING_CLOSE = { li: ['li'], option: ['option'], dt: ['dt', 'dd'], dd: ['dt', 'dd'],
    tr: ['tr', 'td', 'th'], td: ['td', 'th'], th: ['td', 'th'] };

  function parseHTML(html) {
    const root = new DocumentFragment();
    const stack = [root];
    const top = () => stack[stack.length - 1];
    let i = 0;
    const n = html.length;
    while (i < n) {
      if (html.startsWith('<!--', i)) {
        const end = html.indexOf('-->', i + 4);
        const stop = end < 0 ? n : end;
        top().appendChild(new Comment(html.slice(i + 4, stop)));
        i = end < 0 ? n : end + 3;
        continue;
      }
      if (html.startsWith('<!', i) || html.startsWith('<?', i)) {
        const end = html.indexOf('>', i);
        i = end < 0 ? n : end + 1;
        continue;
      }
      if (html.startsWith('</', i)) {
        const m = /^<\/([a-zA-Z][-a-zA-Z0-9_:]*)[^>]*>?/.exec(html.slice(i, i + 256));
        if (!m) { top().appendChild(new Text('</')); i += 2; continue; }
        const name = m[1].toLowerCase();
        for (let k = stack.length - 1; k > 0; k--) {
          if (stack[k].localName === name) { stack.length = k; break; }
        }
        i += m[0].length;
        continue;
      }
      if (html[i] === '<' && /[a-zA-Z]/.test(html[i + 1] || '')) {
        let j = i + 1;
        while (j < n && /[-a-zA-Z0-9_:]/.test(html[j])) j++;
        const name = html.slice(i + 1, j).toLowerCase();
        const el = createElement(name);
        let selfClose = false;
        for (;;) {
          while (j < n && /\s/.test(html[j])) j++;
          if (j >= n) break;
          if (html[j] === '>') { j++; break; }
          if (html.startsWith('/>', j)) { selfClose = true; j += 2; break; }
          let k = j;
          while (k < n && !/[\s=>]/.test(html[k]) && !html.startsWith('/>', k)) k++;
          const an = html.slice(j, k).toLowerCase();
          if (k === j) { j++; continue; }
          j = k;
          while (j < n && /\s/.test(html[j])) j++;
          let val = '';
          if (html[j] === '=') {
            j++;
            while (j < n && /\s/.test(html[j])) j++;
            const q = html[j];
            if (q === '"' || q === "'") {
              const end = html.indexOf(q, j + 1);
              const stop = end < 0 ? n : end;
              val = html.slice(j + 1, stop);
              j = end < 0 ? n : end + 1;
            } else {
              let e = j;
              while (e < n && !/[\s>]/.test(html[e])) e++;
              val = html.slice(j, e);
              j = e;
            }
          }
          if (!el._attrs.has(an)) el._attrs.set(an, decode(val));
        }
        i = j;
        // Implicit ends of the elements that allow them.
        if (CLOSES_P.has(name)) {
          for (let k = stack.length - 1; k > 0; k--) if (stack[k].localName === 'p') { stack.length = k; break; }
        }
        const sib = SIBLING_CLOSE[name];
        if (sib && sib.includes(top().localName)) stack.length -= 1;
        top().appendChild(el);
        if (VOID.has(name) || selfClose) continue;
        if (RAW.has(name) || RCDATA.has(name)) {
          const end = html.toLowerCase().indexOf('</' + name, i);
          const stop = end < 0 ? n : end;
          const text = html.slice(i, stop);
          if (text) el.appendChild(new Text(RAW.has(name) ? text : decode(text)));
          const close = end < 0 ? n : html.indexOf('>', end);
          i = close < 0 ? n : close + 1;
          continue;
        }
        if (stack.length < 256) stack.push(el);
        continue;
      }
      let end = html.indexOf('<', i + (html[i] === '<' ? 1 : 0));
      if (end < 0) end = n;
      top().appendChild(new Text(decode(html.slice(i, end))));
      i = end;
    }
    return root.childNodes.slice();
  }

  const escText = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    .replace(/ /g, '&nbsp;');
  const escAttr = (s) => s.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/ /g, '&nbsp;');

  // `wire`: the copy for the browser (ids in data-kjs, no scripts, no comments, only names its
  // parser reads).
  function serialize(node, wire, out) {
    const top = !out;
    out = out || [];
    switch (node.nodeType) {
      case 1: {
        const tag = node.localName;
        if (wire && (tag === 'script' || tag === 'template' || !NAME_OK.test(tag))) {
          if (tag !== 'script' && tag !== 'template') for (const k of node.childNodes) serialize(k, wire, out);
          break;
        }
        out.push('<', tag);
        if (wire) out.push(' data-kjs="', String(node.uid), '"');
        for (const [k, v] of (wire && hooks.wireAttrs ? hooks.wireAttrs(node) : node._attrs)) {
          if (wire && (k === 'data-kjs' || !NAME_OK.test(k))) continue;
          out.push(' ', k, '="', escAttr(v), '"');
        }
        out.push('>');
        if (VOID.has(tag)) break;
        if (RAW.has(tag)) {
          const t = node.textContent;
          out.push(wire ? t.replace(/<\//g, '<\\/') : t);
        } else {
          for (const k of (wire && hooks.wireKids ? hooks.wireKids(node) : node.childNodes)) serialize(k, wire, out);
        }
        out.push('</', tag, '>');
        break;
      }
      case 3:
        out.push(escText(node._data));
        break;
      case 8:
        if (!wire) out.push('<!--', node._data, '-->');
        break;
      default:
        for (const k of node.childNodes) serialize(k, wire, out);
    }
    return top ? out.join('') : out;
  }

  // ---- the document ----

  class Document extends Node {
    constructor() {
      super(9, '#document');
      this.readyState = 'loading';
      this._quirks = false;
      this._active = null;
      this._cookieText = '';
      this.currentScript = null;
      this.referrer = '';
      this.characterSet = 'UTF-8';
      this.charset = 'UTF-8';
      this.contentType = 'text/html';
      this.visibilityState = 'visible';
      this.hidden = false;
      this.implementation = { hasFeature: () => true, createHTMLDocument: () => document };
    }
    get documentElement() { return this.childNodes.find((n) => n.nodeType === 1) || null; }
    get head() { const h = this.documentElement; return h ? h.childNodes.find((n) => n.localName === 'head') || null : null; }
    get body() { const h = this.documentElement; return h ? h.childNodes.find((n) => n.localName === 'body') || null : null; }
    get title() { const t = this.querySelector('title'); return t ? t.textContent.trim() : ''; }
    set title(v) {
      let t = this.querySelector('title');
      if (!t) {
        t = createElement('title');
        (this.head || this.documentElement || this).appendChild(t);
      }
      t.textContent = v;
    }
    get location() { return location; }
    set location(v) { location.href = v; }
    get defaultView() { return global; }
    get URL() { return location.href; }
    get documentURI() { return location.href; }
    get domain() { return location.hostname; }
    get compatMode() { return this._quirks ? 'BackCompat' : 'CSS1Compat'; }
    get activeElement() { return this._active || this.body; }
    get scrollingElement() { return this.documentElement; }
    get forms() { return this.getElementsByTagName('form'); }
    get images() { return this.getElementsByTagName('img'); }
    get links() { return this.querySelectorAll('a[href]'); }
    get scripts() { return this.getElementsByTagName('script'); }
    // The browser's cookie jar decides (Domain, Path, Secure, HttpOnly, SameSite, limits): a write
    // goes to it ("C" + text) and its answer replaces the value; meanwhile a read sees the write.
    get cookie() { return this._cookieText; }
    set cookie(v) {
      v = String(v);
      send('C' + v);
      const first = v.split(';')[0];
      const i = first.indexOf('=');
      if (i <= 0 || /;\s*httponly\s*(;|$)/i.test(v)) return;
      const name = first.slice(0, i).trim();
      const exp = /;\s*expires\s*=([^;]*)/i.exec(v);
      const gone = /;\s*max-age\s*=\s*(-|0+\s*(;|$))/i.test(v) || (exp && Date.parse(exp[1]) < Date.now());
      const list = this._cookieText ? this._cookieText.split('; ').filter((p) => p.split('=')[0] !== name) : [];
      if (!gone) list.push(name + '=' + first.slice(i + 1).trim());
      this._cookieText = list.join('; ');
    }
    hasFocus() { return true; }
    getElementById(id) {
      id = String(id);
      const walk = (n) => {
        for (const k of n.childNodes) {
          if (k.nodeType !== 1) continue;
          if (k._attrs.get('id') === id) return k;
          const r = walk(k);
          if (r) return r;
        }
        return null;
      };
      return walk(this);
    }
    getElementsByName(name) { return descendants(this).filter((e) => e.getAttribute('name') === String(name)); }
    createElement(tag) { return createElement(tag); }
    createElementNS(_, tag) { return createElement(String(tag).replace(/^.*:/, '')); }
    createTextNode(t) { return new Text(t); }
    createComment(t) { return new Comment(t); }
    createDocumentFragment() { return new DocumentFragment(); }
    createEvent(kind) { return /mouse/i.test(kind) ? new MouseEvent('') : new Event(''); }
    createRange() {
      return { selectNodeContents() {}, setStart() {}, setEnd() {}, collapse() {},
        createContextualFragment(html) { const f = new DocumentFragment(); for (const n of parseHTML(String(html))) f.appendChild(n); return f; },
        getBoundingClientRect: () => ZERO_RECT };
    }
    importNode(n, deep) { return n.cloneNode(deep); }
    adoptNode(n) { if (n.parentNode) n.parentNode._detach(n); return n; }
    write(...parts) {
      const target = this.body || this.documentElement;
      if (target) for (const n of parseHTML(parts.join(''))) target.appendChild(n);
    }
    writeln(...parts) { this.write(parts.join('') + '\n'); }
    open() { return this; }
    close() {}
    elementFromPoint() { return this.body; }
    getSelection() { return global.getSelection(); }
    execCommand() { return false; }
  }
  mixin(Document.prototype, parentMethods);

  const document = new Document();

  // ---- the address ----

  function splitURL(href) {
    const m = /^([a-zA-Z][-a-zA-Z0-9+.]*:)(?:\/\/([^/?#]*))?([^?#]*)(\?[^#]*)?(#.*)?$/.exec(String(href));
    if (!m) return null;
    const host = m[2] || '';
    const hm = /^(?:[^@]*@)?(\[[^\]]*\]|[^:]*)(?::(\d*))?$/.exec(host) || [];
    return { protocol: m[1].toLowerCase(), host, hostname: hm[1] || '', port: hm[2] || '',
      pathname: m[3] || (m[2] !== undefined ? '/' : ''), search: m[4] && m[4] !== '?' ? m[4] : '',
      hash: m[5] && m[5] !== '#' ? m[5] : '' };
  }
  function resolveURL(rel, base) {
    rel = String(rel).trim();
    if (/^[a-zA-Z][-a-zA-Z0-9+.]*:/.test(rel)) return rel;
    const b = splitURL(base);
    if (!b) return rel;
    const origin = b.protocol + '//' + b.host;
    if (rel.startsWith('//')) return b.protocol + rel;
    if (rel.startsWith('#')) return origin + b.pathname + b.search + rel;
    if (rel.startsWith('?')) return origin + b.pathname + rel;
    let path = rel.startsWith('/') ? rel : b.pathname.replace(/[^/]*$/, '') + rel;
    const qi = path.search(/[?#]/);
    const tail = qi < 0 ? '' : path.slice(qi);
    path = qi < 0 ? path : path.slice(0, qi);
    const out = [];
    for (const seg of path.split('/')) {
      if (seg === '..') { if (out.length > 1) out.pop(); } else if (seg !== '.') out.push(seg);
    }
    if (/\/\.\.?$/.test(path)) out.push('');
    return origin + out.join('/') + tail;
  }

  function navigate(url) { send('N' + String(url)); }

  const location = {
    _url: 'about:blank',
    get href() { return this._url; },
    set href(v) { navigate(resolveURL(v, this._url)); },
    _part(k) { const p = splitURL(this._url); return p ? p[k] : ''; },
    get protocol() { return this._part('protocol'); },
    get host() { return this._part('host'); },
    get hostname() { return this._part('hostname'); },
    get port() { return this._part('port'); },
    get pathname() { return this._part('pathname'); },
    get search() { return this._part('search'); },
    get hash() { return this._part('hash'); },
    set hash(v) {
      v = String(v);
      const h = v === '' ? '' : v.startsWith('#') ? v : '#' + v;
      this._url = this._url.replace(/#.*$/, '') + h;
      setTimeout(() => dispatch(global, new Event('hashchange')), 0);
    },
    get origin() { const p = splitURL(this._url); return p ? p.protocol + '//' + p.host : 'null'; },
    assign(u) { this.href = u; },
    replace(u) { this.href = u; },
    reload() { navigate(this._url); },
    toString() { return this._url; },
  };

  class URLSearchParams {
    constructor(init) {
      this._list = [];
      if (typeof init === 'string') {
        for (const part of init.replace(/^\?/, '').split('&')) {
          if (!part) continue;
          const i = part.indexOf('=');
          const dec = (x) => { try { return decodeURIComponent(x.replace(/\+/g, ' ')); } catch (e) { return x; } };
          this._list.push(i < 0 ? [dec(part), ''] : [dec(part.slice(0, i)), dec(part.slice(i + 1))]);
        }
      } else if (init && typeof init === 'object') {
        for (const [k, v] of Array.isArray(init) ? init : Object.entries(init)) this._list.push([String(k), String(v)]);
      }
    }
    get(k) { const e = this._list.find((x) => x[0] === k); return e ? e[1] : null; }
    getAll(k) { return this._list.filter((x) => x[0] === k).map((x) => x[1]); }
    has(k) { return this._list.some((x) => x[0] === k); }
    set(k, v) { this.delete(k); this._list.push([String(k), String(v)]); }
    append(k, v) { this._list.push([String(k), String(v)]); }
    delete(k) { this._list = this._list.filter((x) => x[0] !== k); }
    forEach(fn) { for (const [k, v] of this._list) fn(v, k, this); }
    keys() { return this._list.map((x) => x[0])[Symbol.iterator](); }
    values() { return this._list.map((x) => x[1])[Symbol.iterator](); }
    entries() { return this._list.map((x) => x.slice())[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
    toString() { return this._list.map(([k, v]) => encodeURIComponent(k) + '=' + encodeURIComponent(v)).join('&'); }
  }
  class URL {
    constructor(u, base) {
      const abs = base === undefined ? String(u) : resolveURL(u, String(base));
      const p = splitURL(abs);
      if (!p) throw new TypeError('Invalid URL: ' + u);
      Object.assign(this, p);
      this.searchParams = new URLSearchParams(p.search);
    }
    get origin() { return this.protocol + '//' + this.host; }
    get href() {
      const q = this.searchParams.toString();
      return this.protocol + '//' + this.host + this.pathname + (q ? '?' + q : '') + this.hash;
    }
    toString() { return this.href; }
    toJSON() { return this.href; }
  }

  // ---- timers and tasks ----

  const tasks = [];
  const timers = new Map();
  const frames = new Map();
  let timerSeq = 0;
  let lastFrame = 0;
  let current = null;

  function addTimer(fn, ms, args, every) {
    const id = ++timerSeq;
    ms = Math.max(0, Number(ms) || 0);
    timers.set(id, { id, fn, args, at: now() + ms, every: every ? Math.max(4, ms) : 0 });
    return id;
  }
  function runTimer(t) {
    if (typeof t.fn === 'function') t.fn(...t.args);
    else evalScript(String(t.fn), 'timer');
  }

  function runOne() {
    current = null;
    if (tasks.length) {
      const t = tasks.shift();
      current = t;
      t.run();
      current = null;
      return 1;
    }
    const at = now();
    let due = null;
    for (const t of timers.values()) if (t.at <= at && (!due || t.at < due.at)) due = t;
    if (due) {
      if (due.every) due.at = at + due.every; else timers.delete(due.id);
      current = due;
      try { runTimer(due); } catch (e) { reportError(e); }
      current = null;
      return 1;
    }
    if (frames.size && at - lastFrame >= 16) {
      lastFrame = at;
      const list = [...frames.values()];
      frames.clear();
      for (const cb of list) { try { cb(at); } catch (e) { reportError(e); } }
      return 1;
    }
    return 0;
  }

  function nextDelay() {
    if (tasks.length) return 0;
    const at = now();
    let d = -1;
    for (const t of timers.values()) {
      const w = Math.max(0, Math.ceil(t.at - at));
      if (d < 0 || w < d) d = w;
    }
    if (frames.size) {
      const w = Math.max(0, Math.ceil(lastFrame + 16 - at));
      if (d < 0 || w < d) d = w;
    }
    return d;
  }

  function interrupted() {
    const what = current && current.id ? 'a timer' : current && current.name ? current.name : 'a task';
    console.error('jsrun: ' + what + ' ran for too long and was stopped');
    if (current && current.id) timers.delete(current.id);
    current = null;
  }

  function flush() {
    if (!dirty || !document.documentElement) return;
    dirty = false;
    send('D' + (document._quirks ? '' : '<!doctype html>') + serialize(document.documentElement, true));
  }

  // ---- what the browser sends ----

  function build(parent, list) {
    for (const item of list) {
      if (typeof item === 'string') {
        parent.childNodes.push(new Text(item));
        parent.childNodes[parent.childNodes.length - 1].parentNode = parent;
        continue;
      }
      const [tag, uid, attrs, kids] = item;
      const el = createElement(tag, uid);
      for (const k of Object.keys(attrs)) el._attrs.set(k, String(attrs[k]));
      el.parentNode = parent;
      parent.childNodes.push(el);
      build(el, kids);
    }
  }

  function init(m) {
    location._url = String(m.url);
    navigator.userAgent = String(m.ua);
    navigator.language = String(m.lang || 'pt-BR');
    navigator.languages = [navigator.language];
    global.innerWidth = global.outerWidth = screen.width = screen.availWidth = m.w | 0;
    global.innerHeight = global.outerHeight = screen.height = screen.availHeight = m.h | 0;
    document._quirks = !!m.quirks;
    document._cookieText = String(m.cookie || '');
    build(document, m.dom || []);
    dirty = false;
    for (const [name, code] of m.scripts || []) {
      tasks.push({ name: 'script ' + name, run: () => evalScript(String(code), String(name)) });
    }
    tasks.push({ name: 'DOMContentLoaded', run() {
      document.readyState = 'interactive';
      dispatch(document, new Event('readystatechange'));
      dispatch(document, new Event('DOMContentLoaded', { bubbles: true }));
    } });
    tasks.push({ name: 'load', run() {
      document.readyState = 'complete';
      dispatch(document, new Event('readystatechange'));
      const ev = new Event('load');
      dispatch(global, ev);
      // `<body onload>` is the window's handler.
      const body = document.body;
      const src = body && body.getAttribute('onload');
      if (src && !global.onload) {
        try { new Function('event', src).call(global, ev); } catch (e) { reportError(e); }
      }
      console.log('jsrun: page ready (' + (m.scripts || []).length + ' scripts)');
    } });
  }

  function click(m) {
    const target = byUid.get(m.uid) || document.body || document.documentElement;
    let ok = true;
    try {
      if (target) {
        const ev = new MouseEvent('click', { bubbles: true, cancelable: true, clientX: m.x | 0, clientY: m.y | 0 });
        ev.isTrusted = true;
        ok = dispatch(target, ev);
        const link = ok && target.closest ? target.closest('a[href]') : null;
        const href = link ? link.getAttribute('href').trim() : '';
        if (/^javascript:/i.test(href)) {
          ok = false;
          evalScript(decodeURIComponent(href.slice(11)), 'javascript: link');
        }
        const button = ok && target.closest ? target.closest('button, input[type=submit]') : null;
        const form = button ? button.closest('form') : null;
        const kind = button ? (button.getAttribute('type') || 'submit').toLowerCase() : '';
        if (form && kind === 'submit') {
          ok = dispatch(form, new Event('submit', { bubbles: true, cancelable: true }));
        }
      }
    } finally {
      if (m.seq) send('A' + m.seq + ' ' + (ok ? 1 : 0));
    }
  }

  function onMessage(text) {
    const m = JSON.parse(text);
    if (m.t === 'init') tasks.push({ name: 'init', run: () => init(m) });
    else if (m.t === 'click') tasks.push({ name: 'click', run: () => click(m) });
  }

  // ---- the window ----

  class Storage {
    constructor() { this._m = new Map(); }
    get length() { return this._m.size; }
    key(i) { return [...this._m.keys()][i] ?? null; }
    getItem(k) { return this._m.has(String(k)) ? this._m.get(String(k)) : null; }
    setItem(k, v) { this._m.set(String(k), String(v)); }
    removeItem(k) { this._m.delete(String(k)); }
    clear() { this._m.clear(); }
  }
  class Observer {
    constructor(cb) { this._cb = cb; }
    observe() {}
    unobserve() {}
    disconnect() {}
    takeRecords() { return []; }
  }

  const navigator = {
    userAgent: '', language: 'pt-BR', languages: ['pt-BR'], platform: 'Kitsune x86_64',
    vendor: 'Kitsune', appName: 'Netscape', appVersion: '5.0', product: 'Gecko', onLine: true,
    cookieEnabled: true, hardwareConcurrency: 1, maxTouchPoints: 0, doNotTrack: '1',
    javaEnabled: () => false, sendBeacon: () => false,
  };
  const screen = { width: 0, height: 0, availWidth: 0, availHeight: 0, colorDepth: 24, pixelDepth: 24 };

  const win = {
    window: global, self: global, top: global, parent: global, frames: global, opener: null,
    document, location, navigator, screen, history: {
      length: 1, state: null, scrollRestoration: 'auto',
      pushState(s) { this.state = s; }, replaceState(s) { this.state = s; },
      back() {}, forward() {}, go() {},
    },
    localStorage: new Storage(), sessionStorage: new Storage(),
    innerWidth: 0, innerHeight: 0, outerWidth: 0, outerHeight: 0, devicePixelRatio: 1,
    scrollX: 0, scrollY: 0, pageXOffset: 0, pageYOffset: 0, name: '', closed: false, isSecureContext: true,
    Node, Element, HTMLElement, Text, Comment, Document, DocumentFragment, CharacterData,
    Event, CustomEvent, UIEvent, MouseEvent, KeyboardEvent, FocusEvent, InputEvent, PointerEvent: MouseEvent,
    EventTarget: class EventTarget { constructor() { this._ls = null; } },
    DOMException, DOMTokenList, URL, URLSearchParams, Storage,
    MutationObserver: Observer, IntersectionObserver: Observer, ResizeObserver: Observer,
    setTimeout(fn, ms, ...args) { return addTimer(fn, ms, args, false); },
    setInterval(fn, ms, ...args) { return addTimer(fn, ms, args, true); },
    clearTimeout(id) { timers.delete(id); },
    clearInterval(id) { timers.delete(id); },
    setImmediate(fn, ...args) { return addTimer(fn, 0, args, false); },
    requestAnimationFrame(fn) { const id = ++timerSeq; frames.set(id, fn); return id; },
    cancelAnimationFrame(id) { frames.delete(id); },
    requestIdleCallback(fn) { return addTimer(() => fn({ didTimeout: false, timeRemaining: () => 10 }), 1, [], false); },
    cancelIdleCallback(id) { timers.delete(id); },
    queueMicrotask(fn) { Promise.resolve().then(fn); },
    getComputedStyle(el) {
      return new Proxy({}, {
        get(_, p) {
          if (p === 'getPropertyValue') return (n) => getDecl(el, n);
          if (typeof p !== 'string') return undefined;
          const v = getDecl(el, kebab(p));
          if (v) return v;
          if (p === 'display') return el.hidden ? 'none' : 'block';
          return '';
        },
      });
    },
    matchMedia(q) {
      return { matches: false, media: String(q), onchange: null, addListener() {}, removeListener() {},
        addEventListener() {}, removeEventListener() {} };
    },
    getSelection() { return { rangeCount: 0, toString: () => '', removeAllRanges() {}, addRange() {} }; },
    scrollTo() {}, scrollBy() {}, scroll() {}, focus() {}, blur() {}, print() {}, stop() {}, close() {},
    alert(msg) { console.log('alert: ' + msg); },
    confirm(msg) { console.log('confirm: ' + msg); return false; },
    prompt(msg) { console.log('prompt: ' + msg); return null; },
    open(u) { if (u) location.href = u; return null; },
    postMessage() {},
    atob(s) {
      const A = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
      s = String(s).replace(/[\s=]/g, '');
      let out = '', buf = 0, bits = 0;
      for (const c of s) {
        const v = A.indexOf(c);
        if (v < 0) throw new DOMException('bad base64', 'InvalidCharacterError');
        buf = (buf << 6) | v; bits += 6;
        if (bits >= 8) { bits -= 8; out += String.fromCharCode((buf >> bits) & 255); }
      }
      return out;
    },
    btoa(s) {
      const A = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
      s = String(s);
      let out = '';
      for (let i = 0; i < s.length; i += 3) {
        const a = s.charCodeAt(i), b = s.charCodeAt(i + 1), c = s.charCodeAt(i + 2);
        if (a > 255 || b > 255 || c > 255) throw new DOMException('not Latin-1', 'InvalidCharacterError');
        const n = (a << 16) | ((b || 0) << 8) | (c || 0);
        out += A[n >> 18] + A[(n >> 12) & 63] + (i + 1 < s.length ? A[(n >> 6) & 63] : '=') + (i + 2 < s.length ? A[n & 63] : '=');
      }
      return out;
    },
  };
  for (const k of Object.keys(win)) {
    Object.defineProperty(global, k, Object.getOwnPropertyDescriptor(win, k));
  }
  for (const tag of ['Div', 'Span', 'Anchor', 'Button', 'Input', 'Form', 'Image', 'Paragraph',
    'Heading', 'List', 'LI', 'UList', 'OList', 'Table', 'TableRow', 'TableCell', 'Select', 'Option',
    'TextArea', 'Label', 'Script', 'Style', 'Link', 'Meta', 'Body', 'Head', 'Html', 'Template',
    'Canvas', 'Video', 'Audio', 'IFrame', 'Pre', 'BR', 'HR', 'Title']) {
    global['HTML' + tag + 'Element'] = HTMLElement;
  }
  Object.assign(global.EventTarget.prototype, targetMethods);
  Object.assign(global, targetMethods);
  global._ls = null;
  global._on = null;
  addHandlerProps(global);
  global.__kjs = { onMessage, runOne, nextDelay, flush, interrupted };
  // What the files evaluated after this one build on (they get it as `kjs.dom`).
  global.__kjs.dom = {
    Node, Element, HTMLElement, CharacterData, Text, Comment, DocumentFragment, Document, document,
    Event, CustomEvent, UIEvent, MouseEvent, KeyboardEvent, FocusEvent, InputEvent, DOMException,
    DOMTokenList, dispatch, invoke, handlerOf, byUid, touch, serialize, parseHTML, createElement,
    parseSelector, matchComplex, descendants, tasks, timers, addTimer, reportError, splitURL,
    resolveURL, navigate, location, navigator, screen, Storage, mutators, hooks, getDecl, setDecl,
    kebab, camel, targetMethods, addHandlerProps, HANDLERS, isDirty: () => dirty,
  };
})(globalThis, globalThis.__kjs_native);
