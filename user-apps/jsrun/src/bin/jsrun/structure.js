// Kitsune jsrun --page: the structure of a modern document.
//
// `<template>` and its `content`, the element classes (`HTMLInputElement`...: `instanceof` works),
// `style` as a CSSStyleDeclaration, tree walking (`TreeWalker`, `NodeIterator`), `Range` and
// `Selection` (the parts frameworks use; the browser has no caret in page text), node utilities
// (`compareDocumentPosition`, `normalize`, `splitText`...), custom elements (`customElements.define`,
// upgrades, the lifecycle callbacks) and a basic shadow DOM: `attachShadow`, `<slot>` with names and
// fallback. The browser draws a shadow tree in place of the host's children (the `wireKids` hook):
// the slots take the host's children; a `<style>` in a shadow tree applies to the whole page (no
// scoping, no `:host`).
(function (global, native, kjs) {
  'use strict';
  const D = kjs.dom;
  const { document, Node, Element, DocumentFragment } = D;
  const define = (name, value) => {
    Object.defineProperty(global, name, { value, writable: true, configurable: true, enumerable: false });
  };
  const P = Element.prototype;
  const NP = Node.prototype;

  // ---- element classes ----

  const Base = D.HTMLElement;
  const ce = { defs: new Map(), byCtor: new Map(), waiting: new Map(), upgrading: null };
  class HTMLElement extends Base {
    constructor(tag, uid) {
      if (ce.upgrading) { const el = ce.upgrading; ce.upgrading = null; return el; }
      const def = ce.byCtor.get(new.target);
      super(def ? def.name : tag, uid);
      if (def) this._kjsCE = def;
    }
  }
  define('HTMLElement', HTMLElement);
  const CLASSES = {
    a: 'Anchor', area: 'Area', audio: 'Audio', br: 'BR', base: 'Base', body: 'Body', button: 'Button',
    canvas: 'Canvas', data: 'Data', datalist: 'DataList', details: 'Details', dialog: 'Dialog', div: 'Div',
    dl: 'DList', embed: 'Embed', fieldset: 'FieldSet', form: 'Form', h1: 'Heading', h2: 'Heading',
    h3: 'Heading', h4: 'Heading', h5: 'Heading', h6: 'Heading', head: 'Head', hr: 'HR', html: 'Html',
    iframe: 'IFrame', img: 'Image', input: 'Input', label: 'Label', legend: 'Legend', li: 'LI', link: 'Link',
    map: 'Map', menu: 'Menu', meta: 'Meta', meter: 'Meter', object: 'Object', ol: 'OList', optgroup: 'OptGroup',
    option: 'Option', output: 'Output', p: 'Paragraph', picture: 'Picture', pre: 'Pre', progress: 'Progress',
    q: 'Quote', blockquote: 'Quote', script: 'Script', select: 'Select', slot: 'Slot', source: 'Source',
    span: 'Span', style: 'Style', table: 'Table', caption: 'TableCaption', td: 'TableCell', th: 'TableCell',
    col: 'TableCol', tbody: 'TableSection', thead: 'TableSection', tfoot: 'TableSection', tr: 'TableRow',
    template: 'Template', textarea: 'TextArea', time: 'Time', title: 'Title', ul: 'UList', summary: 'Element',
  };
  // <video>, <audio>, <source> and <track> are left to the media code (media.js).
  const classes = new Map();
  const classOf = (name) => {
    let C = classes.get(name);
    if (!C) {
      C = name === 'Element' ? HTMLElement : class extends HTMLElement {};
      classes.set(name, C);
      if (name !== 'Element') {
        Object.defineProperty(C, 'name', { value: 'HTML' + name + 'Element' });
        define('HTML' + name + 'Element', C);
      }
    }
    return C;
  };
  for (const n of new Set(Object.values(CLASSES).concat(['Unknown']))) classOf(n);
  class SVGElement extends Base {
    get namespaceURI() { return 'http://www.w3.org/2000/svg'; }
    get ownerSVGElement() { return this.closest('svg'); }
    get className() { return this.getAttribute('class') || ''; }
    set className(v) { this.setAttribute('class', v); }
  }
  class SVGSVGElement extends SVGElement {
    createSVGPoint() { return new global.DOMPoint(); }
    createSVGMatrix() { return new global.DOMMatrix(); }
  }
  define('SVGElement', SVGElement);
  define('SVGSVGElement', SVGSVGElement);
  define('SVGGraphicsElement', SVGElement);
  for (const n of ['Path', 'G', 'Rect', 'Circle', 'Line', 'Polygon', 'Polyline', 'Text', 'Use', 'Defs', 'Ellipse']) define('SVG' + n + 'Element', SVGElement);
  Object.defineProperty(P, 'namespaceURI', { configurable: true, get() { return this._kjsSvg ? 'http://www.w3.org/2000/svg' : 'http://www.w3.org/1999/xhtml'; } });
  Object.defineProperty(P, 'prefix', { configurable: true, get() { return null; } });

  const created = D.hooks.created;
  D.hooks.created = function (el) {
    if (created) el = created(el);
    if (Object.getPrototypeOf(el) !== Base.prototype) return el;
    const def = ce.defs.get(el.localName);
    if (def) { upgrade(el, def); return el; }
    const name = CLASSES[el.localName];
    if (/^(video|audio|source|track)$/.test(el.localName)) return el;
    Object.setPrototypeOf(el, (name ? classOf(name) : el.localName.includes('-') ? HTMLElement : classOf('Unknown')).prototype);
    if (el.localName.includes('-')) {
      const list = ce.waiting.get(el.localName) || [];
      if (list.length < 1000) list.push(el);
      ce.waiting.set(el.localName, list);
    }
    return el;
  };
  const createNS = D.Document.prototype.createElementNS;
  D.Document.prototype.createElementNS = function (ns, tag) {
    const el = createNS.call(this, ns, tag);
    if (ns === 'http://www.w3.org/2000/svg') {
      el._kjsSvg = true;
      Object.setPrototypeOf(el, (String(tag).toLowerCase() === 'svg' ? SVGSVGElement : SVGElement).prototype);
    }
    return el;
  };
  const createEl = D.Document.prototype.createElement;
  D.Document.prototype.createElement = function (tag, opts) {
    const is = opts && typeof opts === 'object' ? opts.is : null;
    const def = ce.defs.get(String(tag).toLowerCase());
    if (def && !is) return construct(def);
    return createEl.call(this, tag);
  };

  // ---- custom elements ----

  function construct(def) {
    const el = new def.ctor();
    el._kjsCE = def;
    return el;
  }
  function upgrade(el, def) {
    if (el._kjsCE) return;
    Object.setPrototypeOf(el, def.ctor.prototype);
    el._kjsCE = def;
    ce.upgrading = el;
    try { new def.ctor(); } catch (e) { D.reportError(e); } finally { ce.upgrading = null; }
    for (const a of def.observed) {
      const v = el.getAttribute(a);
      if (v !== null) callback(el, 'attributeChangedCallback', a, null, v);
    }
    if (el.isConnected) { connectedSet.add(el); callback(el, 'connectedCallback'); }
  }
  function callback(el, name, ...args) {
    const f = el[name];
    if (typeof f !== 'function') return;
    try { f.apply(el, args); } catch (e) { D.reportError(e); }
  }
  const NAME = /^[a-z][-.0-9_a-z·À-￿]*-[-.0-9_a-z·À-￿]*$/;
  const customElements = {
    define(name, ctor, opts) {
      name = String(name);
      if (!NAME.test(name)) throw new D.DOMException("'" + name + "' is not a valid custom element name", 'SyntaxError');
      if (ce.defs.has(name)) throw new D.DOMException("'" + name + "' has already been defined", 'NotSupportedError');
      if (typeof ctor !== 'function') throw new TypeError('customElements.define: not a constructor');
      const observed = [].concat(ctor.observedAttributes || []).map((a) => String(a).toLowerCase());
      const def = { name, ctor, observed, ext: opts && opts.extends };
      ce.defs.set(name, def);
      ce.byCtor.set(ctor, def);
      for (const el of ce.waiting.get(name) || []) {
        if (!el._kjsCE && el.localName === name) upgrade(el, def);
      }
      ce.waiting.delete(name);
      const w = whenDefined.get(name);
      if (w) { w.resolve(ctor); }
    },
    get(name) { const d = ce.defs.get(String(name)); return d ? d.ctor : undefined; },
    getName(ctor) { const d = ce.byCtor.get(ctor); return d ? d.name : null; },
    whenDefined(name) {
      name = String(name);
      if (ce.defs.has(name)) return Promise.resolve(ce.defs.get(name).ctor);
      let w = whenDefined.get(name);
      if (!w) {
        let resolve;
        const promise = new Promise((r) => { resolve = r; });
        w = { promise, resolve };
        whenDefined.set(name, w);
      }
      return w.promise;
    },
    upgrade(root) { for (const el of [root, ...D.descendants(root)]) { const d = el.nodeType === 1 && ce.defs.get(el.localName); if (d) upgrade(el, d); } },
  };
  const whenDefined = new Map();
  define('customElements', customElements);
  define('CustomElementRegistry', function CustomElementRegistry() {});
  // Lifecycle: connected/disconnected on insertion and removal, attributeChanged for the observed ones.
  const connectedSet = new WeakSet();
  function walkCustom(n, f) {
    if (n.nodeType === 1) {
      if (n._kjsCE) f(n);
      if (n._kjsShadow) for (const k of n._kjsShadow.childNodes.slice()) walkCustom(k, f);
    }
    for (const k of n.childNodes.slice()) walkCustom(k, f);
  }
  D.mutators.push(function (type, target, a, b) {
    if (!ce.defs.size) return;
    if (type === 'childList') {
      for (const n of b) walkCustom(n, (el) => { if (connectedSet.has(el) && !el.isConnected) { connectedSet.delete(el); callback(el, 'disconnectedCallback'); } });
      for (const n of a) walkCustom(n, (el) => { if (el.isConnected && !connectedSet.has(el)) { connectedSet.add(el); callback(el, 'connectedCallback'); } });
    } else if (type === 'attributes' && target._kjsCE && target._kjsCE.observed.includes(a)) {
      callback(target, 'attributeChangedCallback', a, b, target.getAttribute(a));
    }
  });

  // ---- shadow DOM ----

  class ShadowRoot extends DocumentFragment {
    constructor(host, init) {
      super();
      this.nodeName = '#document-fragment';
      this._kjsHost = host;
      this.mode = init && init.mode === 'closed' ? 'closed' : 'open';
      this.delegatesFocus = !!(init && init.delegatesFocus);
      this.slotAssignment = (init && init.slotAssignment) || 'named';
      this.clonable = !!(init && init.clonable);
    }
    get host() { return this._kjsHost; }
    get innerHTML() { return this.childNodes.map((n) => D.serialize(n, false)).join(''); }
    set innerHTML(html) {
      this._dropAll();
      for (const n of D.parseHTML(String(html))) this.appendChild(n);
      D.touch();
    }
    get activeElement() { const a = document._active; return a && this.contains(a) ? a : null; }
    getSelection() { return global.getSelection(); }
    elementFromPoint(x, y) { return document.elementFromPoint(x, y); }
  }
  define('ShadowRoot', ShadowRoot);
  P.attachShadow = function (init) {
    if (this._kjsShadow) throw new D.DOMException('the element already has a shadow root', 'NotSupportedError');
    const root = new ShadowRoot(this, init);
    this._kjsShadow = root;
    D.touch();
    return root;
  };
  Object.defineProperty(P, 'shadowRoot', { configurable: true, get() { return this._kjsShadow && this._kjsShadow.mode === 'open' ? this._kjsShadow : null; } });
  Object.defineProperty(NP, 'isConnected', {
    configurable: true,
    get() {
      let n = this;
      for (let i = 0; i < 4096 && n; i++) {
        if (n === document) return true;
        n = n.parentNode || n._kjsHost || null;
      }
      return false;
    },
  });
  NP.getRootNode = function (opts) {
    let n = this;
    for (;;) {
      if (n.parentNode) { n = n.parentNode; continue; }
      if (n._kjsHost && opts && opts.composed) { n = n._kjsHost; continue; }
      return n;
    }
  };
  // The nodes a slot shows: the host's children assigned to it, else its own (fallback) content.
  function assigned(slot) {
    let root = slot;
    while (root.parentNode) root = root.parentNode;
    const host = root._kjsHost;
    if (!host) return null;
    const name = slot.getAttribute('name') || '';
    return host.childNodes.filter((n) => (n.nodeType === 1 ? (n.getAttribute('slot') || '') : '') === name && (n.nodeType !== 3 || name === ''));
  }
  Object.assign(P, {
    assignedNodes(o) { if (this.localName !== 'slot') return []; const a = assigned(this) || []; return a.length || !(o && o.flatten) ? a : this.childNodes.slice(); },
    assignedElements(o) { return this.assignedNodes(o).filter((n) => n.nodeType === 1); },
    assign() {},
  });
  Object.defineProperty(NP, 'assignedSlot', {
    configurable: true,
    get() {
      const host = this.parentNode;
      if (!host || !host._kjsShadow) return null;
      const name = this.nodeType === 1 ? this.getAttribute('slot') || '' : '';
      return D.descendants(host._kjsShadow).find((s) => s.localName === 'slot' && (s.getAttribute('name') || '') === name) || null;
    },
  });
  Object.defineProperty(P, 'slot', { configurable: true, get() { return this.getAttribute('slot') || ''; }, set(v) { this.setAttribute('slot', v); } });
  // What the browser draws inside `el`: its shadow tree if it has one, where each <slot> is replaced by
  // the nodes assigned to it (or its fallback content).
  const wireKids = D.hooks.wireKids;
  function flatten(list, out, depth) {
    for (const k of list) {
      if (k.nodeType === 1 && k.localName === 'slot' && depth < 32) {
        const a = assigned(k);
        if (a !== null) { flatten(a.length ? a : k.childNodes, out, depth + 1); continue; }
      }
      out.push(k);
    }
    return out;
  }
  D.hooks.wireKids = function (el) {
    if (el._kjsShadow) return flatten(el._kjsShadow.childNodes, [], 0);
    const kids = wireKids ? wireKids(el) : el.childNodes;
    return kids.some((k) => k.localName === 'slot') ? flatten(kids, [], 0) : kids;
  };

  // ---- <template> ----

  Object.defineProperty(P, 'content', {
    configurable: true,
    get() {
      if (this.localName !== 'template') return undefined;
      if (!this._kjsContent) {
        this._kjsContent = new DocumentFragment();
        for (const k of this.childNodes.slice()) this._kjsContent.appendChild(k);
      }
      return this._kjsContent;
    },
  });
  const innerHTML = Object.getOwnPropertyDescriptor(P, 'innerHTML');
  Object.defineProperty(P, 'innerHTML', {
    configurable: true,
    get() { return this.localName === 'template' ? this.content.childNodes.map((n) => D.serialize(n, false)).join('') : innerHTML.get.call(this); },
    set(v) {
      if (this.localName !== 'template') { innerHTML.set.call(this, v); return; }
      const c = this.content;
      c._dropAll();
      for (const n of D.parseHTML(String(v))) c.appendChild(n);
    },
  });
  const cloneNode = NP.cloneNode;
  NP.cloneNode = function (deep) {
    const c = cloneNode.call(this, deep);
    if (this.nodeType === 1 && this.localName === 'template' && deep && this._kjsContent) {
      for (const k of this._kjsContent.childNodes) c.content.appendChild(k.cloneNode(true));
    }
    if (this._kjsSvg) { c._kjsSvg = true; Object.setPrototypeOf(c, Object.getPrototypeOf(this)); }
    return c;
  };

  // ---- style as a CSSStyleDeclaration ----

  const decls = (el) => {
    const out = [];
    for (const part of (el.getAttribute('style') || '').split(';')) {
      const i = part.indexOf(':');
      if (i > 0) out.push([part.slice(0, i).trim().toLowerCase(), part.slice(i + 1).trim()]);
    }
    return out;
  };
  function writeDecls(el, list) {
    const text = list.map((d) => d[0] + ': ' + d[1]).join('; ');
    if (text) el.setAttribute('style', text + ';'); else if (el.hasAttribute('style')) el.setAttribute('style', '');
  }
  function setProperty(el, name, value, prio) {
    name = String(name).trim();
    if (!name.startsWith('--')) name = name.toLowerCase();
    const list = decls(el).filter((d) => d[0] !== name);
    if (value !== null && value !== undefined && String(value) !== '') {
      list.push([name, String(value) + (prio === 'important' ? ' !important' : '')]);
    }
    writeDecls(el, list);
  }
  const styles = new WeakMap();
  function styleOf(el) {
    let s = styles.get(el);
    if (s) return s;
    const api = {
      setProperty: (n, v, p) => setProperty(el, n, v, p),
      getPropertyValue: (n) => { const d = decls(el).find((x) => x[0] === String(n).trim().toLowerCase() || x[0] === String(n).trim()); return d ? d[1].replace(/\s*!important$/, '') : ''; },
      getPropertyPriority: (n) => { const d = decls(el).find((x) => x[0] === String(n).trim().toLowerCase()); return d && /!important$/.test(d[1]) ? 'important' : ''; },
      removeProperty: (n) => { const old = api.getPropertyValue(n); setProperty(el, n, ''); return old; },
      item: (i) => (decls(el)[i] || [''])[0],
    };
    s = new Proxy(api, {
      get(t, p) {
        if (typeof p !== 'string') return p === Symbol.iterator ? function* () { for (const d of decls(el)) yield d[0]; } : undefined;
        if (p in t) return t[p];
        if (p === 'cssText') return el.getAttribute('style') || '';
        if (p === 'length') return decls(el).length;
        if (p === 'parentRule') return null;
        if (p === 'cssFloat') return api.getPropertyValue('float');
        if (/^\d+$/.test(p)) return api.item(+p);
        return api.getPropertyValue(D.kebab(p));
      },
      set(t, p, v) {
        if (typeof p !== 'string') return true;
        if (p === 'cssText') el.setAttribute('style', String(v));
        else if (p === 'cssFloat') setProperty(el, 'float', v);
        else setProperty(el, D.kebab(p), v);
        return true;
      },
      has(t, p) { return typeof p === 'string'; },
    });
    styles.set(el, s);
    return s;
  }
  Object.defineProperty(P, 'style', { configurable: true, get() { return styleOf(this); }, set(v) { this.setAttribute('style', String(v)); } });
  define('CSSStyleDeclaration', function CSSStyleDeclaration() {});
  const CSSStyleSheet = class { constructor() { this.cssRules = []; } insertRule(r, i) { this.cssRules.splice(i || 0, 0, { cssText: String(r) }); return i || 0; } deleteRule(i) { this.cssRules.splice(i, 1); } replaceSync() {} };
  define('CSSStyleSheet', CSSStyleSheet);
  Object.defineProperty(D.Document.prototype, 'styleSheets', { configurable: true, get() { return []; } });

  // ---- nodes ----

  const order = (n) => { const path = []; for (; n; n = n.parentNode) path.unshift(n); return path; };
  Object.assign(NP, {
    compareDocumentPosition(o) {
      if (o === this) return 0;
      const a = order(this), b = order(o);
      if (a[0] !== b[0]) return 1 | 32 | 2;
      if (this.contains(o)) return 16 | 4;
      if (o.contains(this)) return 8 | 2;
      let i = 0;
      while (a[i] === b[i]) i++;
      const p = a[i - 1].childNodes;
      return p.indexOf(a[i]) < p.indexOf(b[i]) ? 4 : 2;
    },
    normalize() {
      for (let i = 0; i < this.childNodes.length; i++) {
        const k = this.childNodes[i];
        if (k.nodeType === 3) {
          while (this.childNodes[i + 1] && this.childNodes[i + 1].nodeType === 3) {
            k._data += this.childNodes[i + 1]._data;
            this._detach(this.childNodes[i + 1]);
          }
          if (!k._data) { this._detach(k); i--; }
        } else if (k.nodeType === 1) k.normalize();
      }
    },
    lookupNamespaceURI() { return null; },
    lookupPrefix() { return null; },
    isDefaultNamespace() { return true; },
  });
  Object.defineProperty(NP, 'baseURI', { configurable: true, get() { return location.href; } });
  const CD = D.CharacterData.prototype;
  Object.assign(CD, {
    substringData(o, n) { return this._data.substr(o, n); },
    insertData(o, s) { this.data = this._data.slice(0, o) + s + this._data.slice(o); },
    deleteData(o, n) { this.data = this._data.slice(0, o) + this._data.slice(o + n); },
    replaceData(o, n, s) { this.data = this._data.slice(0, o) + s + this._data.slice(o + n); },
  });
  D.Text.prototype.splitText = function (o) {
    const rest = new D.Text(this._data.slice(o));
    this.data = this._data.slice(0, o);
    if (this.parentNode) this.parentNode.insertBefore(rest, this.nextSibling);
    return rest;
  };
  const attrsDesc = Object.getOwnPropertyDescriptor(P, 'attributes');
  Object.defineProperty(P, 'attributes', {
    configurable: true,
    get() {
      const list = attrsDesc.get.call(this);
      const el = this;
      list.getNamedItem = (n) => list.find((a) => a.name === String(n).toLowerCase()) || null;
      list.item = (i) => list[i] || null;
      list.removeNamedItem = (n) => el.removeAttribute(n);
      list.setNamedItem = (a) => el.setAttribute(a.name, a.value);
      return list;
    },
  });
  P.getAttributeNode = function (n) { const v = this.getAttribute(n); return v === null ? null : { name: String(n).toLowerCase(), value: v, nodeName: n, specified: true }; };
  P.getElementsByTagNameNS = function (_, t) { return this.getElementsByTagName(t); };
  P.animate = function () {
    const a = { playState: 'finished', currentTime: 0, onfinish: null, oncancel: null, effect: null,
      play() {}, pause() {}, cancel() {}, finish() {}, reverse() {}, commitStyles() {}, persist() {},
      addEventListener() {}, removeEventListener() {} };
    a.finished = Promise.resolve(a);
    a.ready = Promise.resolve(a);
    global.setTimeout(() => { if (typeof a.onfinish === 'function') a.onfinish({ target: a }); }, 0);
    return a;
  };
  P.getAnimations = function () { return []; };
  P.requestFullscreen = function () { return Promise.reject(new D.DOMException('no full screen', 'NotSupportedError')); };
  P.requestPointerLock = function () {};
  P.hasPointerCapture = function () { return false; };
  P.setPointerCapture = function () {};
  P.releasePointerCapture = function () {};
  Object.defineProperty(P, 'isContentEditable', { configurable: true, get() { const c = this.closest('[contenteditable]'); return !!c && c.getAttribute('contenteditable') !== 'false'; } });
  Object.defineProperty(P, 'contentEditable', { configurable: true, get() { return this.getAttribute('contenteditable') ?? 'inherit'; }, set(v) { this.setAttribute('contenteditable', v); } });
  for (const [k, a] of [['draggable', 'draggable'], ['spellcheck', 'spellcheck'], ['inert', 'inert'], ['translate', 'translate']]) {
    Object.defineProperty(P, k, { configurable: true, get() { return this.hasAttribute(a) && this.getAttribute(a) !== 'false'; }, set(v) { this.toggleAttribute(a, !!v); } });
  }
  for (const k of ['accessKey', 'enterKeyHint', 'inputMode', 'nonce', 'popover']) {
    const a = k.toLowerCase();
    Object.defineProperty(P, k, { configurable: true, get() { return this.getAttribute(a) || ''; }, set(v) { this.setAttribute(a, v); } });
  }
  Object.defineProperty(P, 'open', { configurable: true, get() { return this.hasAttribute('open'); }, set(v) { this.toggleAttribute('open', !!v); } });
  Object.assign(P, {
    showModal() { this.setAttribute('open', ''); },
    show() { this.setAttribute('open', ''); },
    close(v) { if (v !== undefined) this.returnValue = v; this.removeAttribute('open'); this.dispatchEvent(new Event('close')); },
  });
  const DTL = D.DOMTokenList.prototype;
  Object.assign(DTL, {
    supports() { return true; },
    entries() { return this._get().entries(); },
    keys() { return this._get().keys(); },
    values() { return this._get().values(); },
  });

  // ---- tree walking ----

  const NodeFilter = {
    FILTER_ACCEPT: 1, FILTER_REJECT: 2, FILTER_SKIP: 3, SHOW_ALL: 0xffffffff, SHOW_ELEMENT: 1, SHOW_ATTRIBUTE: 2,
    SHOW_TEXT: 4, SHOW_CDATA_SECTION: 8, SHOW_PROCESSING_INSTRUCTION: 64, SHOW_COMMENT: 128, SHOW_DOCUMENT: 256,
    SHOW_DOCUMENT_TYPE: 512, SHOW_DOCUMENT_FRAGMENT: 1024,
  };
  define('NodeFilter', NodeFilter);
  function accept(w, n) {
    if (!((w.whatToShow >>> 0) & (1 << (n.nodeType - 1)))) return 3;
    const f = w.filter;
    if (!f) return 1;
    const r = typeof f === 'function' ? f(n) : f.acceptNode(n);
    return r === undefined ? 1 : r;
  }
  class TreeWalker {
    constructor(root, what, filter) { this.root = root; this.whatToShow = what === undefined ? 0xffffffff : what; this.filter = filter || null; this.currentNode = root; }
    parentNode() {
      for (let n = this.currentNode; n && n !== this.root;) {
        n = n.parentNode;
        if (n && accept(this, n) === 1) return (this.currentNode = n);
      }
      return null;
    }
    _first(last) {
      let n = last ? this.currentNode.lastChild : this.currentNode.firstChild;
      while (n) {
        const r = accept(this, n);
        if (r === 1) return (this.currentNode = n);
        if (r === 3) { const c = last ? n.lastChild : n.firstChild; if (c) { n = c; continue; } }
        for (;;) {
          const s = last ? n.previousSibling : n.nextSibling;
          if (s) { n = s; break; }
          n = n.parentNode;
          if (!n || n === this.root || n === this.currentNode) return null;
        }
      }
      return null;
    }
    firstChild() { return this._first(false); }
    lastChild() { return this._first(true); }
    _sibling(prev) {
      let n = this.currentNode;
      if (n === this.root) return null;
      for (;;) {
        let s = prev ? n.previousSibling : n.nextSibling;
        while (s) {
          const r = accept(this, s);
          if (r === 1) return (this.currentNode = s);
          const c = prev ? s.lastChild : s.firstChild;
          if (r === 3 && c) { s = c; continue; }
          s = prev ? s.previousSibling : s.nextSibling;
        }
        n = n.parentNode;
        if (!n || n === this.root || accept(this, n) === 1) return null;
      }
    }
    nextSibling() { return this._sibling(false); }
    previousSibling() { return this._sibling(true); }
    nextNode() {
      let n = this.currentNode;
      for (;;) {
        let r = 1;
        while (r !== 2 && n.firstChild) {
          n = n.firstChild;
          r = accept(this, n);
          if (r === 1) return (this.currentNode = n);
        }
        let s = null;
        for (let t = n; t && t !== this.root; t = t.parentNode) { s = t.nextSibling; if (s) break; }
        if (!s) return null;
        n = s;
        r = accept(this, n);
        if (r === 1) return (this.currentNode = n);
        if (r === 2) { // skip its children
          let s2 = null;
          for (let t = n; t && t !== this.root; t = t.parentNode) { s2 = t.nextSibling; if (s2) break; }
          if (!s2) return null;
          n = s2;
          if (accept(this, n) === 1) return (this.currentNode = n);
        }
      }
    }
    previousNode() {
      let n = this.currentNode;
      while (n !== this.root) {
        let s = n.previousSibling;
        while (s) {
          n = s;
          let r = accept(this, n);
          while (r !== 2 && n.lastChild) { n = n.lastChild; r = accept(this, n); }
          if (r === 1) return (this.currentNode = n);
          s = n.previousSibling;
        }
        if (n === this.root || !n.parentNode) return null;
        n = n.parentNode;
        if (accept(this, n) === 1) return (this.currentNode = n);
      }
      return null;
    }
  }
  class NodeIterator {
    constructor(root, what, filter) { this._w = new TreeWalker(root, what, filter); this.root = root; this.whatToShow = this._w.whatToShow; this.filter = this._w.filter; this.referenceNode = root; this._started = false; }
    nextNode() {
      if (!this._started) { this._started = true; if (accept(this._w, this.root) === 1) return this.root; }
      const n = this._w.nextNode();
      if (n) this.referenceNode = n;
      return n;
    }
    previousNode() { const n = this._w.previousNode(); if (n) this.referenceNode = n; return n; }
    detach() {}
  }
  define('TreeWalker', TreeWalker);
  define('NodeIterator', NodeIterator);
  D.Document.prototype.createTreeWalker = (root, what, filter) => new TreeWalker(root, what, filter);
  D.Document.prototype.createNodeIterator = (root, what, filter) => new NodeIterator(root, what, filter);

  // ---- Range and Selection ----

  const lengthOf = (n) => (n.nodeType === 3 || n.nodeType === 8 ? n._data.length : n.childNodes.length);
  class Range {
    constructor() { this.startContainer = this.endContainer = document; this.startOffset = this.endOffset = 0; }
    get collapsed() { return this.startContainer === this.endContainer && this.startOffset === this.endOffset; }
    get commonAncestorContainer() {
      for (let n = this.startContainer; n; n = n.parentNode) if (n.contains(this.endContainer)) return n;
      return document;
    }
    setStart(n, o) { this.startContainer = n; this.startOffset = o; if (this._after(this.startContainer, o, this.endContainer, this.endOffset)) this.collapse(true); }
    setEnd(n, o) { this.endContainer = n; this.endOffset = o; if (this._after(this.startContainer, this.startOffset, n, o)) this.collapse(false); }
    _after(a, ao, b, bo) {
      if (a === b) return ao > bo;
      const pos = a.compareDocumentPosition(b);
      return !!(pos & 2) && !(pos & 8);
    }
    setStartBefore(n) { this.setStart(n.parentNode, n.parentNode.childNodes.indexOf(n)); }
    setStartAfter(n) { this.setStart(n.parentNode, n.parentNode.childNodes.indexOf(n) + 1); }
    setEndBefore(n) { this.setEnd(n.parentNode, n.parentNode.childNodes.indexOf(n)); }
    setEndAfter(n) { this.setEnd(n.parentNode, n.parentNode.childNodes.indexOf(n) + 1); }
    selectNode(n) { this.setStartBefore(n); this.setEndAfter(n); }
    selectNodeContents(n) { this.startContainer = this.endContainer = n; this.startOffset = 0; this.endOffset = lengthOf(n); }
    collapse(toStart) { if (toStart) { this.endContainer = this.startContainer; this.endOffset = this.startOffset; } else { this.startContainer = this.endContainer; this.startOffset = this.endOffset; } }
    cloneRange() { const r = new Range(); Object.assign(r, { startContainer: this.startContainer, startOffset: this.startOffset, endContainer: this.endContainer, endOffset: this.endOffset }); return r; }
    // The nodes inside the range, whole (text nodes cut at the ends are handled by the callers).
    _nodes() {
      const out = [];
      const root = this.commonAncestorContainer;
      const w = new TreeWalker(root, 0xffffffff);
      for (let n = root; n; n = w.nextNode()) if (n.nodeType === 3 && this.intersectsNode(n)) out.push(n);
      return out;
    }
    toString() {
      if (this.startContainer === this.endContainer && this.startContainer.nodeType === 3) return this.startContainer._data.slice(this.startOffset, this.endOffset);
      return this._nodes().map((t) => {
        let s = t._data;
        if (t === this.endContainer) s = s.slice(0, this.endOffset);
        if (t === this.startContainer) s = s.slice(this.startOffset);
        return s;
      }).join('');
    }
    intersectsNode(n) {
      const p = n.parentNode;
      if (!p) return true;
      const i = p.childNodes.indexOf(n);
      return !this._after(p, i, this.startContainer, this.startOffset) && !this._after(this.endContainer, this.endOffset, p, i + 1) ? true
        : this.startContainer === n || this.endContainer === n || n.contains(this.startContainer) || (!this._after(p, i + 1, this.startContainer, this.startOffset) && !this._after(this.endContainer, this.endOffset, p, i));
    }
    isPointInRange(n, o) { return !this._after(this.startContainer, this.startOffset, n, o) && !this._after(n, o, this.endContainer, this.endOffset); }
    comparePoint(n, o) { return this._after(this.startContainer, this.startOffset, n, o) ? -1 : this._after(n, o, this.endContainer, this.endOffset) ? 1 : 0; }
    compareBoundaryPoints(how, r) {
      const [a, ao] = how === 0 || how === 3 ? [this.startContainer, this.startOffset] : [this.endContainer, this.endOffset];
      const [b, bo] = how === 0 || how === 1 ? [r.startContainer, r.startOffset] : [r.endContainer, r.endOffset];
      return a === b && ao === bo ? 0 : this._after(a, ao, b, bo) ? 1 : -1;
    }
    insertNode(n) {
      const c = this.startContainer;
      if (c.nodeType === 3) { const rest = c.splitText(this.startOffset); c.parentNode.insertBefore(n, rest); }
      else c.insertBefore(n, c.childNodes[this.startOffset] || null);
    }
    extractContents() {
      const f = new DocumentFragment();
      if (this.startContainer === this.endContainer) {
        const c = this.startContainer;
        if (c.nodeType === 3) { f.appendChild(new D.Text(c._data.slice(this.startOffset, this.endOffset))); c.deleteData(this.startOffset, this.endOffset - this.startOffset); }
        else for (const k of c.childNodes.slice(this.startOffset, this.endOffset)) f.appendChild(k);
        this.collapse(true);
        return f;
      }
      for (const t of this._nodes()) f.appendChild(t);
      this.collapse(true);
      return f;
    }
    cloneContents() {
      const f = new DocumentFragment();
      if (this.startContainer === this.endContainer && this.startContainer.nodeType !== 3) {
        for (const k of this.startContainer.childNodes.slice(this.startOffset, this.endOffset)) f.appendChild(k.cloneNode(true));
      } else f.appendChild(new D.Text(this.toString()));
      return f;
    }
    deleteContents() { this.extractContents(); }
    surroundContents(el) { el.appendChild(this.extractContents()); this.insertNode(el); this.selectNode(el); }
    createContextualFragment(html) { const f = new DocumentFragment(); for (const n of D.parseHTML(String(html))) f.appendChild(n); return f; }
    getBoundingClientRect() {
      let el = this.startContainer.nodeType === 1 ? this.startContainer.childNodes[this.startOffset] || this.startContainer : this.startContainer.parentNode;
      while (el && el.nodeType !== 1) el = el.parentNode;
      return el && kjs.layout ? kjs.layout.clientRect(el) : new global.DOMRect();
    }
    getClientRects() { return [this.getBoundingClientRect()]; }
    detach() {}
  }
  Object.assign(Range, { START_TO_START: 0, START_TO_END: 1, END_TO_END: 2, END_TO_START: 3 });
  define('Range', Range);
  define('StaticRange', Range);
  define('AbstractRange', Range);
  D.Document.prototype.createRange = () => new Range();

  class Selection {
    constructor() { this._r = null; }
    get rangeCount() { return this._r ? 1 : 0; }
    get anchorNode() { return this._r ? this._r.startContainer : null; }
    get anchorOffset() { return this._r ? this._r.startOffset : 0; }
    get focusNode() { return this._r ? this._r.endContainer : null; }
    get focusOffset() { return this._r ? this._r.endOffset : 0; }
    get isCollapsed() { return !this._r || this._r.collapsed; }
    get type() { return !this._r ? 'None' : this._r.collapsed ? 'Caret' : 'Range'; }
    getRangeAt(i) { if (i !== 0 || !this._r) throw new D.DOMException('no range', 'IndexSizeError'); return this._r; }
    addRange(r) { if (!this._r) this._r = r; }
    removeRange() { this._r = null; }
    removeAllRanges() { this._r = null; }
    empty() { this._r = null; }
    collapse(n, o) { if (!n) { this._r = null; return; } const r = new Range(); r.setStart(n, o || 0); this._r = r; }
    collapseToStart() { if (this._r) this._r.collapse(true); }
    collapseToEnd() { if (this._r) this._r.collapse(false); }
    extend(n, o) { if (this._r) this._r.setEnd(n, o || 0); }
    setBaseAndExtent(a, ao, b, bo) { const r = new Range(); r.setStart(a, ao); r.setEnd(b, bo); this._r = r; }
    selectAllChildren(n) { const r = new Range(); r.selectNodeContents(n); this._r = r; }
    containsNode(n) { return !!this._r && this._r.intersectsNode(n); }
    deleteFromDocument() { if (this._r) this._r.deleteContents(); }
    modify() {}
    toString() { return this._r ? this._r.toString() : ''; }
  }
  const selection = new Selection();
  define('Selection', Selection);
  define('getSelection', () => selection);
  D.Document.prototype.getSelection = () => selection;
})(globalThis, globalThis.__kjs_native, globalThis.__kjs);
