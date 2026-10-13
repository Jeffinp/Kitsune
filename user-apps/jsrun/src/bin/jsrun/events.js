// Kitsune jsrun --page: what the user does, as the page's scripts see it.
//
// Evaluated after dom.js and net.js. The browser keeps the controls on screen (the caret, the text
// typed, a check box ticked) and tells this process every key, focus change and control change, with
// the control's new state; here they become `keydown`/`keypress`/`input`/`keyup`, `focus`/`blur`/
// `focusin`/`focusout`, `input`/`change`, `click` with its default actions (check boxes, labels, links,
// form submission with `submit`), and the mouse events (`mousedown`/`mouseup`/`click`/`dblclick`/
// `contextmenu`/`wheel`, and `mousemove`/`mouseover`/`mouseout`/`mouseenter`/`mouseleave` only while
// the page listens to them: the browser is told with a `W` message which kinds the page wants).
// The state of a control lives here (`value`, `checked`, `selectedIndex`); the browser's copy of the
// document carries it, so what a script sets shows on screen. See docs/design/tls-browser.md §14.
//
// Messages (kitsune_core::web::script::input): from the browser `key`, `focus`, `field`, `mouse`,
// `click`, `vis`; to the browser `Q<n>` (the inputs up to n are in the document that follows), `G<uid>`
// (a script focused a control; 0 = none), `W<kinds>` (the events the page listens to).
(function (global, native, kjs) {
  'use strict';
  const D = kjs.dom;
  const { document, dispatch, byUid, touch, Element, Node, reportError } = D;
  const send = native.send;
  const define = (name, value) => {
    Object.defineProperty(global, name, { value, writable: true, configurable: true, enumerable: false });
  };

  // ---- event classes ----

  const BaseEvent = D.Event;
  const BaseUI = D.UIEvent;
  const BaseMouse = D.MouseEvent;
  const mods = (o, init) => {
    o.ctrlKey = !!init.ctrlKey; o.shiftKey = !!init.shiftKey; o.altKey = !!init.altKey; o.metaKey = !!init.metaKey;
  };
  const modState = function (k) {
    return { Control: this.ctrlKey, Shift: this.shiftKey, Alt: this.altKey, Meta: this.metaKey }[k] || false;
  };
  class UIEvent extends BaseUI {
    constructor(type, init) {
      super(type, init);
      init = init || {};
      this.detail = init.detail || 0;
      this.view = init.view === undefined ? global : init.view;
      this.which = init.which || 0;
    }
  }
  class FocusEvent extends UIEvent {
    constructor(type, init) { super(type, init); this.relatedTarget = (init && init.relatedTarget) || null; }
  }
  class MouseEvent extends BaseMouse {
    constructor(type, init) {
      super(type, init);
      init = init || {};
      this.screenX = init.screenX ?? this.clientX;
      this.screenY = init.screenY ?? this.clientY;
      this.movementX = init.movementX || 0;
      this.movementY = init.movementY || 0;
      this.detail = init.detail || 0;
      this.view = init.view === undefined ? global : init.view;
      this.which = this.button + 1;
      this.pageX = this.x = this.clientX;
      this.pageY = this.y = this.clientY;
      this.layerX = this.offsetX = this.clientX;
      this.layerY = this.offsetY = this.clientY;
    }
    getModifierState(k) { return modState.call(this, k); }
    initMouseEvent(type, bubbles, cancelable, view, detail, sx, sy, cx, cy, ctrl, alt, shift, meta, button, rel) {
      this.initEvent(type, bubbles, cancelable);
      Object.assign(this, { detail, screenX: sx, screenY: sy, clientX: cx, clientY: cy, ctrlKey: ctrl,
        altKey: alt, shiftKey: shift, metaKey: meta, button, relatedTarget: rel || null });
    }
  }
  class PointerEvent extends MouseEvent {
    constructor(type, init) {
      super(type, init);
      init = init || {};
      this.pointerId = init.pointerId ?? 1;
      this.pointerType = init.pointerType || 'mouse';
      this.isPrimary = init.isPrimary ?? true;
      this.width = init.width || 1; this.height = init.height || 1;
      this.pressure = init.pressure ?? (this.buttons ? 0.5 : 0);
      this.tiltX = 0; this.tiltY = 0; this.twist = 0; this.tangentialPressure = 0;
    }
    getCoalescedEvents() { return [this]; }
    getPredictedEvents() { return []; }
  }
  class WheelEvent extends MouseEvent {
    constructor(type, init) {
      super(type, init);
      init = init || {};
      this.deltaX = init.deltaX || 0; this.deltaY = init.deltaY || 0; this.deltaZ = init.deltaZ || 0;
      this.deltaMode = init.deltaMode || 0;
      this.wheelDelta = -this.deltaY * 1.2;
    }
  }
  WheelEvent.DOM_DELTA_PIXEL = 0; WheelEvent.DOM_DELTA_LINE = 1; WheelEvent.DOM_DELTA_PAGE = 2;
  class KeyboardEvent extends UIEvent {
    constructor(type, init) {
      super(type, init);
      init = init || {};
      this.key = init.key === undefined ? '' : String(init.key);
      this.code = init.code === undefined ? '' : String(init.code);
      this.location = init.location || 0;
      this.repeat = !!init.repeat;
      this.isComposing = !!init.isComposing;
      mods(this, init);
      this.keyCode = init.keyCode || 0;
      this.charCode = init.charCode || 0;
      this.which = init.which || this.keyCode || this.charCode;
    }
    getModifierState(k) { return modState.call(this, k); }
    initKeyboardEvent(type, bubbles, cancelable, view, key) { this.initEvent(type, bubbles, cancelable); this.key = key || ''; }
  }
  Object.assign(KeyboardEvent, { DOM_KEY_LOCATION_STANDARD: 0, DOM_KEY_LOCATION_LEFT: 1, DOM_KEY_LOCATION_RIGHT: 2, DOM_KEY_LOCATION_NUMPAD: 3 });
  class InputEvent extends UIEvent {
    constructor(type, init) {
      super(type, init);
      init = init || {};
      this.data = init.data ?? null;
      this.inputType = init.inputType || '';
      this.isComposing = !!init.isComposing;
      this.dataTransfer = null;
    }
    getTargetRanges() { return []; }
  }
  class CompositionEvent extends UIEvent {
    constructor(type, init) { super(type, init); this.data = (init && init.data) || ''; }
  }
  class SubmitEvent extends BaseEvent {
    constructor(type, init) { super(type, init); this.submitter = (init && init.submitter) || null; }
  }
  const simple = (fields) => class extends BaseEvent {
    constructor(type, init) {
      super(type, init);
      init = init || {};
      for (const [k, d] of Object.entries(fields)) this[k] = init[k] === undefined ? d : init[k];
    }
  };
  const PopStateEvent = simple({ state: null, hasUAVisualTransition: false });
  const HashChangeEvent = simple({ oldURL: '', newURL: '' });
  const PageTransitionEvent = simple({ persisted: false });
  const MessageEvent = simple({ data: null, origin: '', lastEventId: '', source: null, ports: [] });
  const ErrorEvent = simple({ message: '', filename: '', lineno: 0, colno: 0, error: null });
  const StorageEvent = simple({ key: null, oldValue: null, newValue: null, url: '', storageArea: null });
  const AnimationEvent = simple({ animationName: '', elapsedTime: 0, pseudoElement: '' });
  const TransitionEvent = simple({ propertyName: '', elapsedTime: 0, pseudoElement: '' });
  const PromiseRejectionEvent = simple({ promise: null, reason: undefined });
  const BeforeUnloadEvent = simple({});
  const ClipboardEvent = simple({ clipboardData: null });
  const DragEvent = class extends MouseEvent {
    constructor(type, init) { super(type, init); this.dataTransfer = (init && init.dataTransfer) || null; }
  };
  for (const [k, v] of Object.entries({
    UIEvent, FocusEvent, MouseEvent, PointerEvent, WheelEvent, KeyboardEvent, InputEvent, CompositionEvent,
    SubmitEvent, PopStateEvent, HashChangeEvent, PageTransitionEvent, MessageEvent, ErrorEvent, StorageEvent,
    AnimationEvent, TransitionEvent, PromiseRejectionEvent, BeforeUnloadEvent, ClipboardEvent, DragEvent,
  })) define(k, v);
  const createEvent = document.createEvent;
  document.createEvent = function (kind) {
    const k = String(kind).toLowerCase().replace(/s$/, '');
    const C = { mouseevent: MouseEvent, keyboardevent: KeyboardEvent, uievent: UIEvent, focusevent: FocusEvent,
      customevent: D.CustomEvent, event: BaseEvent, htmlevent: BaseEvent, inputevent: InputEvent,
      messageevent: MessageEvent, pointerevent: PointerEvent, wheelevent: WheelEvent }[k];
    if (!C) return createEvent.call(this, kind);
    const ev = new C('');
    if (ev instanceof D.CustomEvent) ev.initCustomEvent = function (t, b, c, d) { this.initEvent(t, b, c); this.detail = d; };
    return ev;
  };

  // ---- listeners: once, passive, signal, and telling the browser what the page listens to ----

  // The kinds of events the browser sends only when asked for (see `want`).
  const KIND = {
    mousemove: 'move', mouseover: 'move', mouseout: 'move', mouseenter: 'move', mouseleave: 'move',
    pointermove: 'move', pointerover: 'move', pointerout: 'move', pointerenter: 'move', pointerleave: 'move',
    keydown: 'key', keyup: 'key', keypress: 'key', wheel: 'wheel', mousewheel: 'wheel', scroll: 'scroll',
    contextmenu: 'ctx', dblclick: 'dbl',
  };
  const wanted = new Set();
  function want(kind) {
    if (!kind || wanted.has(kind)) return;
    wanted.add(kind);
    send('W' + [...wanted].sort().join(','));
  }
  kjs.want = want;

  const baseAdd = D.targetMethods.addEventListener;
  const baseRemove = D.targetMethods.removeEventListener;
  function addEventListener(type, fn, opts) {
    if (!fn) return;
    type = String(type);
    const signal = opts && typeof opts === 'object' ? opts.signal : null;
    if (signal && signal.aborted) return;
    baseAdd.call(this, type, fn, opts);
    const list = this._ls && this._ls[type];
    const l = list && list[list.length - 1];
    if (l && l.fn === fn && opts && typeof opts === 'object' && opts.passive) l.passive = true;
    if (signal && typeof signal.addEventListener === 'function') {
      const capture = typeof opts === 'boolean' ? opts : !!(opts && opts.capture);
      signal.addEventListener('abort', () => baseRemove.call(this, type, fn, capture), { once: true });
    }
    want(KIND[type]);
  }
  for (const target of [Node.prototype, global.EventTarget.prototype]) {
    Object.defineProperty(target, 'addEventListener', { value: addEventListener, writable: true, configurable: true });
  }
  // The window's own methods work unbound (`addEventListener(...)` in a script).
  for (const [k, f] of [['addEventListener', addEventListener], ['removeEventListener', baseRemove], ['dispatchEvent', D.targetMethods.dispatchEvent]]) {
    Object.defineProperty(global, k, { value: function (...a) { return f.apply(this == null ? global : this, a); }, writable: true, configurable: true });
  }
  D.targetMethods.addEventListener = addEventListener;
  // `el.onmousemove = f` asks for the events too.
  for (const type of Object.keys(KIND).concat(['mousedown', 'mouseup', 'focusin', 'focusout', 'beforeinput', 'animationend', 'transitionend', 'pointerdown', 'pointerup', 'pointercancel', 'focus', 'blur', 'input', 'change', 'toggle', 'message', 'storage', 'pagehide', 'pageshow', 'beforeunload', 'unload', 'visibilitychange', 'select', 'selectionchange', 'paste', 'copy', 'cut'])) {
    const kind = KIND[type];
    const prop = {
      configurable: true,
      get() { return (this._on && this._on[type]) || null; },
      set(fn) {
        if (!this._on) this._on = Object.create(null);
        this._on[type] = typeof fn === 'function' ? fn : null;
        if (fn) want(kind);
      },
    };
    Object.defineProperty(Node.prototype, 'on' + type, prop);
    Object.defineProperty(global, 'on' + type, prop);
  }
  global.onmessage = null;
  const setAttribute = Element.prototype.setAttribute;
  Element.prototype.setAttribute = function (n, v) {
    setAttribute.call(this, n, v);
    const a = String(n).toLowerCase();
    if (a.startsWith('on')) want(KIND[a.slice(2)]);
  };
  function scanHandlers(root) {
    for (const e of D.descendants(root)) {
      for (const a of e._attrs.keys()) if (a.startsWith('on')) want(KIND[a.slice(2)]);
    }
  }

  // ---- the controls' state ----

  const isInput = (e) => e && e.nodeType === 1 && e.localName === 'input';
  const inputType = (e) => {
    const t = (e.getAttribute('type') || '').trim().toLowerCase();
    return t || 'text';
  };
  const toggles = (e) => isInput(e) && (inputType(e) === 'checkbox' || inputType(e) === 'radio');
  const isControl = (e) => e && e.nodeType === 1 && /^(input|textarea|select|button)$/.test(e.localName);
  const textLike = (e) => (isInput(e) && !/^(checkbox|radio|submit|button|reset|image|file|hidden|range|color)$/.test(inputType(e))) || (e && e.localName === 'textarea');
  const optionsOf = (sel) => D.descendants(sel).filter((o) => o.localName === 'option');
  const selectOf = (opt) => opt.closest('select');
  const valueOf = (e) => {
    if (e._kjsV !== undefined) return e._kjsV;
    if (e.localName === 'textarea') return e.textContent;
    return e.getAttribute('value') ?? '';
  };

  const P = Element.prototype;
  const prop = (name, get, set) => Object.defineProperty(P, name, { configurable: true, get, set });
  prop('value', function () {
    if (this.localName === 'select') {
      const o = optionsOf(this)[this.selectedIndex];
      return o ? o.value : '';
    }
    if (this.localName === 'option') return this.getAttribute('value') ?? this.textContent.replace(/\s+/g, ' ').trim();
    if (this.localName === 'input' || this.localName === 'textarea') {
      const v = valueOf(this);
      if (toggles(this) && v === '' && !this.hasAttribute('value')) return 'on';
      return this.localName === 'input' && inputType(this) !== 'hidden' ? v.replace(/[\r\n]/g, '') : v;
    }
    if (this.localName === 'button' || this.localName === 'data' || this.localName === 'param' || this.localName === 'li') {
      return this.getAttribute('value') ?? '';
    }
    return this._kjsV === undefined ? undefined : this._kjsV;
  }, function (v) {
    v = v === null || v === undefined ? '' : String(v);
    if (this.localName === 'select') {
      const i = optionsOf(this).findIndex((o) => o.value === v);
      this.selectedIndex = i;
      return;
    }
    if (this.localName === 'option' || this.localName === 'button' || this.localName === 'li' || this.localName === 'data' || this.localName === 'param') {
      this.setAttribute('value', v);
      return;
    }
    if (this.localName !== 'input' && this.localName !== 'textarea') { this._kjsV = v; return; }
    if (toggles(this)) { this.setAttribute('value', v); return; }
    if (this._kjsV === v) return;
    this._kjsV = v;
    touch();
  });
  prop('defaultValue', function () {
    return this.localName === 'textarea' ? this.textContent : this.getAttribute('value') ?? '';
  }, function (v) {
    if (this.localName === 'textarea') this.textContent = v; else this.setAttribute('value', v);
  });
  prop('checked', function () {
    return this._kjsC !== undefined ? this._kjsC : this.hasAttribute('checked');
  }, function (v) {
    v = !!v;
    if (this.checked === v && this._kjsC !== undefined) return;
    this._kjsC = v;
    if (v && isInput(this) && inputType(this) === 'radio') uncheckGroup(this);
    touch();
  });
  prop('defaultChecked', function () { return this.hasAttribute('checked'); }, function (v) { this.toggleAttribute('checked', !!v); });
  prop('indeterminate', function () { return !!this._kjsInd; }, function (v) { this._kjsInd = !!v; });
  function uncheckGroup(r) {
    const name = r.getAttribute('name');
    if (!name) return;
    const scope = r.closest('form') || r.getRootNode();
    for (const o of D.descendants(scope)) {
      if (o !== r && isInput(o) && inputType(o) === 'radio' && o.getAttribute('name') === name && o.checked) {
        o._kjsC = false;
      }
    }
  }
  prop('selectedIndex', function () {
    if (this.localName !== 'select') return -1;
    const opts = optionsOf(this);
    if (this._kjsSel !== undefined) return this._kjsSel < opts.length ? this._kjsSel : (opts.length ? 0 : -1);
    const i = opts.findIndex((o) => o.hasAttribute('selected'));
    return i >= 0 ? i : (opts.length && !this.multiple ? 0 : -1);
  }, function (i) {
    i = Number(i) | 0;
    if (this._kjsSel === i) return;
    this._kjsSel = i;
    touch();
  });
  prop('selected', function () {
    if (this.localName !== 'option') return undefined;
    const s = selectOf(this);
    if (!s) return this.hasAttribute('selected');
    if (s.multiple) return this._kjsOpt !== undefined ? this._kjsOpt : this.hasAttribute('selected');
    return optionsOf(s)[s.selectedIndex] === this;
  }, function (v) {
    const s = selectOf(this);
    if (!s) { this.toggleAttribute('selected', !!v); return; }
    if (s.multiple) { this._kjsOpt = !!v; touch(); return; }
    if (v) s.selectedIndex = optionsOf(s).indexOf(this);
    else if (this.selected) s.selectedIndex = -1;
  });
  prop('defaultSelected', function () { return this.hasAttribute('selected'); }, function (v) { this.toggleAttribute('selected', !!v); });
  prop('options', function () { return this.localName === 'select' ? optionsOf(this) : undefined; });
  prop('selectedOptions', function () { return this.localName === 'select' ? optionsOf(this).filter((o) => o.selected) : undefined; });
  prop('index', function () { const s = this.localName === 'option' && selectOf(this); return s ? optionsOf(s).indexOf(this) : 0; });
  prop('label', function () { return this.getAttribute('label') ?? this.textContent.trim(); }, function (v) { this.setAttribute('label', v); });
  prop('multiple', function () { return this.hasAttribute('multiple'); }, function (v) { this.toggleAttribute('multiple', !!v); });
  prop('type', function () {
    if (this.localName === 'input') return inputType(this);
    if (this.localName === 'button') return (this.getAttribute('type') || 'submit').toLowerCase();
    if (this.localName === 'select') return this.multiple ? 'select-multiple' : 'select-one';
    if (this.localName === 'textarea') return 'textarea';
    return this.getAttribute('type') ?? '';
  }, function (v) { this.setAttribute('type', v); });
  prop('valueAsNumber', function () { const n = parseFloat(this.value); return Number.isNaN(n) ? NaN : n; }, function (n) { this.value = String(n); });
  prop('length', function () { return this.localName === 'select' ? optionsOf(this).length : this.localName === 'form' ? formControls(this).length : undefined; });
  prop('form', function () {
    if (!isControl(this) && this.localName !== 'fieldset' && this.localName !== 'output' && this.localName !== 'label' && this.localName !== 'option') return undefined;
    const id = this.getAttribute('form');
    if (id) return document.getElementById(id);
    return this.closest('form');
  });
  prop('labels', function () {
    if (!isControl(this)) return undefined;
    return D.descendants(document).filter((l) => l.localName === 'label' && l.control === this);
  });
  prop('control', function () {
    if (this.localName !== 'label') return undefined;
    const f = this.getAttribute('for');
    if (f) { const c = document.getElementById(f); return isControl(c) ? c : null; }
    return D.descendants(this).find(isControl) || null;
  });
  prop('validity', function () {
    const bad = this.required && isControl(this) && !this.value;
    return { valid: !bad, valueMissing: bad, typeMismatch: false, patternMismatch: false, tooLong: false,
      tooShort: false, rangeUnderflow: false, rangeOverflow: false, stepMismatch: false, badInput: false, customError: !!this._kjsValidity };
  });
  prop('validationMessage', function () { return this._kjsValidity || ''; });
  prop('willValidate', function () { return isControl(this); });
  prop('required', function () { return this.hasAttribute('required'); }, function (v) { this.toggleAttribute('required', !!v); });
  prop('readOnly', function () { return this.hasAttribute('readonly'); }, function (v) { this.toggleAttribute('readonly', !!v); });
  prop('autofocus', function () { return this.hasAttribute('autofocus'); }, function (v) { this.toggleAttribute('autofocus', !!v); });
  prop('maxLength', function () { const n = parseInt(this.getAttribute('maxlength'), 10); return Number.isNaN(n) ? -1 : n; }, function (v) { this.setAttribute('maxlength', v); });
  prop('elements', function () {
    if (this.localName !== 'form' && this.localName !== 'fieldset') return undefined;
    const list = formControls(this);
    list.namedItem = (n) => list.find((e) => e.id === n || e.getAttribute('name') === n) || null;
    for (const e of list) { const n = e.getAttribute('name') || e.id; if (n && !(n in list)) list[n] = e; }
    return list;
  });
  prop('selectionStart', function () { return textLike(this) ? (this._kjsSelStart ?? this.value.length) : null; }, function (v) { this._kjsSelStart = v; });
  prop('selectionEnd', function () { return textLike(this) ? (this._kjsSelEnd ?? this.value.length) : null; }, function (v) { this._kjsSelEnd = v; });
  prop('selectionDirection', function () { return 'none'; }, function () {});
  Object.assign(P, {
    checkValidity() { return this.validity.valid; },
    reportValidity() { return this.validity.valid; },
    setCustomValidity(m) { this._kjsValidity = String(m || ''); },
    setSelectionRange(a, b) { this._kjsSelStart = a; this._kjsSelEnd = b; },
    setRangeText(t) { this.value = String(t); },
    select() { this._kjsSelStart = 0; this._kjsSelEnd = (this.value || '').length; dispatch(this, new BaseEvent('select', { bubbles: true })); },
    stepUp(n) { this.value = String((parseFloat(this.value) || 0) + (n || 1) * (parseFloat(this.getAttribute('step')) || 1)); },
    stepDown(n) { this.stepUp(-(n || 1)); },
    showPicker() {},
    add(opt, before) { if (this.localName === 'select') this.insertBefore(opt, typeof before === 'number' ? optionsOf(this)[before] || null : before || null); },
    item(i) { return this.localName === 'select' ? optionsOf(this)[i] || null : null; },
    namedItem(n) { return this.localName === 'select' ? optionsOf(this).find((o) => o.id === n || o.getAttribute('name') === n) || null : null; },
  });
  function formControls(form) {
    const own = D.descendants(form).filter((e) => isControl(e) || e.localName === 'fieldset' || e.localName === 'output');
    const id = form.id;
    if (id) for (const e of D.descendants(document)) if (e.getAttribute('form') === id && !own.includes(e)) own.push(e);
    return own;
  }

  // What the browser's copy of a control carries: its live state, not its attributes.
  D.hooks.wireAttrs = function (el) {
    const n = el.localName;
    if (n === 'input') {
      if (el._kjsV === undefined && el._kjsC === undefined) return el._attrs;
      const a = new Map(el._attrs);
      if (el._kjsV !== undefined && !toggles(el)) a.set('value', el._kjsV);
      if (el._kjsC !== undefined) { if (el._kjsC) a.set('checked', ''); else a.delete('checked'); }
      return a;
    }
    if (n === 'option') {
      const s = selectOf(el);
      if (!s || (s._kjsSel === undefined && el._kjsOpt === undefined)) return el._attrs;
      const a = new Map(el._attrs);
      if (el.selected) a.set('selected', ''); else a.delete('selected');
      return a;
    }
    return el._attrs;
  };
  const wireKids = D.hooks.wireKids;
  D.hooks.wireKids = function (el) {
    if (el.localName === 'textarea' && el._kjsV !== undefined) return [new D.Text(el._kjsV)];
    return wireKids ? wireKids(el) : el.childNodes;
  };

  // A clone keeps the state of a control (as browsers do).
  const cloneNode = Node.prototype.cloneNode;
  Node.prototype.cloneNode = function (deep) {
    const c = cloneNode.call(this, deep);
    if (this.nodeType === 1) {
      if (this._kjsV !== undefined) c._kjsV = this._kjsV;
      if (this._kjsC !== undefined) c._kjsC = this._kjsC;
    }
    return c;
  };

  // ---- focus ----

  const FOCUSABLE = /^(input|textarea|select|button|iframe|summary)$/;
  function focusable(e) {
    if (!e || e.nodeType !== 1) return false;
    if (e.hasAttribute('disabled') && isControl(e)) return false;
    if (FOCUSABLE.test(e.localName)) return !(isInput(e) && inputType(e) === 'hidden');
    if (e.localName === 'a' && e.hasAttribute('href')) return true;
    if (e.hasAttribute('tabindex')) return true;
    return e.getAttribute('contenteditable') === 'true' || e.getAttribute('contenteditable') === '';
  }
  // Move the focus to `el` (null: none) with its events. `fromBrowser`: the browser moved it (the user
  // clicked or pressed Tab); otherwise a script did, and the browser is told so the caret follows.
  function moveFocus(el, fromBrowser) {
    const old = document._active || null;
    if (old === el) return;
    if (old) {
      changeIfEdited(old);
      document._active = null;
      fire(old, new FocusEvent('blur', { relatedTarget: el }));
      fire(old, new FocusEvent('focusout', { bubbles: true, relatedTarget: el }));
    }
    document._active = el;
    if (el) {
      el._kjsFocusV = textLike(el) ? el.value : undefined;
      fire(el, new FocusEvent('focus', { relatedTarget: old }));
      fire(el, new FocusEvent('focusin', { bubbles: true, relatedTarget: old }));
    }
    if (!fromBrowser) send('G' + (el && isControl(el) ? el.uid : 0));
  }
  function changeIfEdited(el) {
    if (textLike(el) && el._kjsFocusV !== undefined && el.value !== el._kjsFocusV) {
      el._kjsFocusV = el.value;
      fire(el, new BaseEvent('change', { bubbles: true }));
    }
  }
  function fire(target, ev) {
    try { return dispatch(target, ev); } catch (e) { reportError(e); return true; }
  }
  Object.assign(P, {
    focus() { if (focusable(this) && this.isConnected) moveFocus(this, false); },
    blur() { if (document._active === this) moveFocus(null, false); },
  });
  Object.defineProperty(P, 'tabIndex', {
    configurable: true,
    get() { const n = parseInt(this.getAttribute('tabindex'), 10); return Number.isNaN(n) ? (focusable(this) ? 0 : -1) : n; },
    set(v) { this.setAttribute('tabindex', v); },
  });
  Object.defineProperty(D.Document.prototype, 'activeElement', {
    configurable: true,
    get() { const a = this._active; return a && a.isConnected ? a : this.body || this.documentElement; },
  });
  D.Document.prototype.hasFocus = function () { return pageVisible; };

  // ---- default actions of a click ----

  const ACTIVATES = 'a[href], area[href], button, input, label, summary, select, textarea';
  // A click on `target` (from the browser, `trusted`, or `el.click()`): the events and what the click
  // does unless prevented. Returns whether the default action may happen (the browser's verdict).
  function activate(target, init, trusted) {
    const act = target.closest ? target.closest(ACTIVATES) : null;
    let undo = null;
    if (act && toggles(act) && !act.disabled) {
      const was = act.checked;
      const radio = inputType(act) === 'radio';
      const others = radio ? D.descendants(act.closest('form') || document).filter((o) => o.checked && o !== act && toggles(o)) : [];
      act.checked = radio ? true : !was;
      undo = () => { act.checked = was; for (const o of others) o.checked = true; };
      undo.was = was;
    }
    if (act && act.localName === 'button' && act.disabled) return false;
    const ev = new PointerEvent('click', init);
    ev.isTrusted = !!trusted;
    const ok = fire(target, ev);
    if (!ok) { if (undo) undo(); return false; }
    if (!act) return true;
    if (undo) {
      if (act.checked !== undo.was) {
        fire(act, new InputEvent('input', { bubbles: true, composed: true }));
        fire(act, new BaseEvent('change', { bubbles: true }));
      }
      return true;
    }
    switch (act.localName) {
      case 'label': {
        const c = act.control;
        if (c && c !== target && !c.contains(target)) {
          if (textLike(c) || c.localName === 'select') c.focus();
          else activate(c, Object.assign({}, init), false);
        }
        return true;
      }
      case 'summary': {
        const d = act.parentNode;
        if (d && d.localName === 'details') { d.toggleAttribute('open'); fire(d, new BaseEvent('toggle')); }
        return true;
      }
      case 'button':
      case 'input': {
        const kind = act.type;
        const form = act.form;
        if (kind === 'submit' || kind === 'image') return form ? submitForm(form, act, !trusted) : true;
        if (kind === 'reset' && form) form.reset();
        return true;
      }
      case 'a':
      case 'area': {
        const href = act.getAttribute('href').trim();
        if (/^javascript:/i.test(href)) {
          native.evalScript(decodeURIComponent(href.slice(11)), 'javascript: link');
          return false;
        }
        // A fragment of this page: an entry of this document (history.js), never a new load.
        const abs = D.resolveURL(href, location.href);
        if (/#/.test(abs) && abs.replace(/#.*$/, '') === location.href.replace(/#.*$/, '') && !act.hasAttribute('download')) {
          if (kjs.goHash) kjs.goHash(abs, false);
          return false;
        }
        if (!trusted) {
          if (href.startsWith('#')) { location.hash = href; return true; }
          if (act.getAttribute('target') === '_blank' && global.open) global.open(act.href);
          else location.assign(act.href);
        }
        return true;
      }
      default:
        return true;
    }
  }
  kjs.activate = activate;

  // The `submit` event of `form`; unless prevented and when `go`, the form is sent (a script's
  // `requestSubmit`, `el.click()`). Returns whether it may be sent.
  function submitForm(form, submitter, go) {
    const ok = fire(form, new SubmitEvent('submit', { bubbles: true, cancelable: true, submitter: submitter || null }));
    if (ok && go) form.submit();
    return ok;
  }
  function formQuery(form, submitter) {
    const q = new URLSearchParams();
    for (const e of formControls(form)) {
      const name = e.getAttribute('name');
      if (!name || e.disabled || e.localName === 'fieldset' || e.localName === 'output') continue;
      const t = e.type;
      if ((t === 'submit' || t === 'image' || e.localName === 'button') && e !== submitter) continue;
      if ((t === 'checkbox' || t === 'radio') && !e.checked) continue;
      if (t === 'reset' || t === 'button' || t === 'file') continue;
      if (e.localName === 'select' && e.multiple) { for (const o of e.selectedOptions) q.append(name, o.value); continue; }
      q.append(name, e.value);
    }
    return q;
  }
  Object.assign(P, {
    click() {
      if (this._kjsInClick) return;
      this._kjsInClick = true;
      try { activate(this, { bubbles: true, cancelable: true, composed: true, detail: 1 }, false); } finally { this._kjsInClick = false; }
    },
    submit() {
      if (this.localName !== 'form') return;
      const action = this.getAttribute('action') || location.href;
      const url = new URL(action, location.href);
      const q = formQuery(this).toString();
      if ((this.getAttribute('method') || 'get').toLowerCase() === 'post') {
        console.warn('jsrun: form.submit() with POST is not supported; sent as GET');
      }
      location.assign(url.origin + url.pathname + (q ? '?' + q : '') + url.hash);
    },
    requestSubmit(submitter) { if (this.localName === 'form') submitForm(this, submitter, true); },
    reset() {
      if (this.localName !== 'form') return;
      if (!fire(this, new BaseEvent('reset', { bubbles: true, cancelable: true }))) return;
      for (const e of formControls(this)) {
        e._kjsV = undefined; e._kjsC = undefined; e._kjsSel = undefined; e._kjsOpt = undefined;
      }
      touch();
    },
  });

  // ---- what the browser sends ----

  let ack = 0;        // the last input the browser numbered that was handled
  let ackSent = 0;
  let hover = null;   // the element under the pointer (move events)
  let pageVisible = true;
  const handlers = Object.create(null);
  kjs.handlers = handlers;

  const keyInit = (m, type) => {
    const bits = m.m | 0;
    const key = String(m.k || '');
    const code = String(m.c || '');
    const init = { bubbles: true, cancelable: true, composed: true, key, code, keyCode: m.kc | 0,
      which: m.kc | 0, shiftKey: !!(bits & 1), ctrlKey: !!(bits & 2), altKey: !!(bits & 4), metaKey: !!(bits & 8),
      repeat: !!m.r };
    if (type === 'keypress') { init.charCode = key.length === 1 ? key.charCodeAt(0) : 13; init.keyCode = init.charCode; init.which = init.charCode; }
    return new KeyboardEvent(type, init);
  };
  const inputKind = (key) => key === 'Backspace' ? 'deleteContentBackward' : key === 'Delete' ? 'deleteContentForward'
    : key === 'Enter' ? 'insertLineBreak' : 'insertText';

  handlers.key = function (m) {
    let el = (m.uid && byUid.get(m.uid)) || null;
    if (el && document._active !== el && el.isConnected) moveFocus(el, true);
    const target = el || document._active || document.body || document.documentElement;
    if (!target) return answer(m, false);
    const down = keyInit(m, 'keydown');
    let ok = fire(target, down);
    const key = String(m.k || '');
    if (ok && (key.length === 1 || key === 'Enter') && !(m.m & 2)) ok = fire(target, keyInit(m, 'keypress'));
    if (el && m.v !== undefined) {
      if (ok) {
        const old = el.value;
        el._kjsV = String(m.v);
        if (old !== el._kjsV) {
          fire(el, new InputEvent('input', { bubbles: true, composed: true, inputType: inputKind(key), data: key.length === 1 ? key : null }));
        }
      } else {
        touch(); // the browser's copy goes back to what the page has
      }
    }
    let allow = ok;
    if (key === 'Enter' && el && ok) {
      if (textLike(el) && el.localName === 'input') changeIfEdited(el);
      if (m.seq) {
        const form = el.form;
        allow = form && el.localName !== 'textarea' ? submitForm(form, null, false) : false;
      }
    }
    fire(target, keyInit(m, 'keyup'));
    answer(m, allow);
  };

  handlers.focus = function (m) {
    const el = (m.uid && byUid.get(m.uid)) || null;
    moveFocus(el && el.isConnected ? el : null, true);
  };

  handlers.field = function (m) {
    const el = byUid.get(m.uid);
    if (!el) return;
    if (m.ck !== undefined && toggles(el)) {
      // Space on a check box: the browser ticked it; here it is the click it stands for.
      el._kjsC = !m.ck;
      activate(el, { bubbles: true, cancelable: true, composed: true, detail: 0 }, true);
      return;
    }
    if (m.si !== undefined && el.localName === 'select') {
      if (el.selectedIndex === (m.si | 0)) return;
      el._kjsSel = m.si | 0;
      fire(el, new InputEvent('input', { bubbles: true, composed: true }));
      fire(el, new BaseEvent('change', { bubbles: true }));
      return;
    }
    if (m.v !== undefined && textLike(el)) {
      if (el.value === String(m.v)) return;
      el._kjsV = String(m.v);
      fire(el, new InputEvent('input', { bubbles: true, composed: true, inputType: 'insertFromPaste' }));
    }
  };

  const mouseInit = (m, extra) => Object.assign({
    bubbles: true, cancelable: true, composed: true, clientX: +m.x || 0, clientY: +m.y || 0,
    button: m.b | 0, buttons: 0, detail: 1, shiftKey: !!(m.m & 1), ctrlKey: !!(m.m & 2), altKey: !!(m.m & 4), metaKey: !!(m.m & 8),
  }, extra);

  handlers.click = function (m) {
    const target = byUid.get(m.uid) || document.body || document.documentElement;
    let ok = true;
    try {
      if (target) {
        const pt = { x: m.cx ?? m.x, y: m.cy ?? m.y, b: 0, m: m.m };
        fire(target, new PointerEvent('pointerdown', mouseInit(pt, { buttons: 1 })));
        const down = fire(target, new MouseEvent('mousedown', mouseInit(pt, { buttons: 1 })));
        if (down && !(target.closest && target.closest('input, textarea, select, button'))) {
          // A click elsewhere moves the focus to what can take it (or to nothing).
          let f = target;
          while (f && f.nodeType === 1 && !focusable(f)) f = f.parentNode;
          moveFocus(f && f.nodeType === 1 ? f : null, true);
        }
        fire(target, new PointerEvent('pointerup', mouseInit(pt)));
        fire(target, new MouseEvent('mouseup', mouseInit(pt)));
        ok = activate(target, mouseInit(pt, { detail: m.d || 1 }), true);
      }
    } finally {
      if (m.seq) send('A' + m.seq + ' ' + (ok ? 1 : 0));
    }
  };

  handlers.mouse = function (m) {
    const target = byUid.get(m.uid) || document.body || document.documentElement;
    if (!target) return;
    switch (m.e) {
      case 'move': {
        if (target !== hover) {
          const old = hover && hover.isConnected ? hover : null;
          hover = target;
          if (old) {
            fire(old, new PointerEvent('pointerout', mouseInit(m, { relatedTarget: target })));
            fire(old, new MouseEvent('mouseout', mouseInit(m, { relatedTarget: target })));
            for (let e = old; e && e.nodeType === 1 && !e.contains(target); e = e.parentNode) {
              fire(e, new PointerEvent('pointerleave', mouseInit(m, { bubbles: false, cancelable: false, relatedTarget: target })));
              fire(e, new MouseEvent('mouseleave', mouseInit(m, { bubbles: false, cancelable: false, relatedTarget: target })));
            }
          }
          fire(target, new PointerEvent('pointerover', mouseInit(m, { relatedTarget: old })));
          fire(target, new MouseEvent('mouseover', mouseInit(m, { relatedTarget: old })));
          const enter = [];
          for (let e = target; e && e.nodeType === 1 && !(old && e.contains(old)); e = e.parentNode) enter.unshift(e);
          for (const e of enter) {
            fire(e, new PointerEvent('pointerenter', mouseInit(m, { bubbles: false, cancelable: false, relatedTarget: old })));
            fire(e, new MouseEvent('mouseenter', mouseInit(m, { bubbles: false, cancelable: false, relatedTarget: old })));
          }
        }
        fire(target, new PointerEvent('pointermove', mouseInit(m)));
        fire(target, new MouseEvent('mousemove', mouseInit(m)));
        break;
      }
      case 'dbl':
        fire(target, new MouseEvent('dblclick', mouseInit(m, { detail: 2 })));
        break;
      case 'ctx':
        fire(target, new PointerEvent('pointerdown', mouseInit(m, { button: 2, buttons: 2 })));
        fire(target, new MouseEvent('mousedown', mouseInit(m, { button: 2, buttons: 2 })));
        fire(target, new MouseEvent('mouseup', mouseInit(m, { button: 2 })));
        fire(target, new PointerEvent('contextmenu', mouseInit(m, { button: 2 })));
        break;
      case 'wheel':
        fire(target, new WheelEvent('wheel', mouseInit(m, { deltaY: +m.dy || 0, deltaMode: 0 })));
        break;
    }
  };

  handlers.vis = function (m) {
    const v = !!m.v;
    if (v === pageVisible) return;
    pageVisible = v;
    document.visibilityState = v ? 'visible' : 'hidden';
    document.hidden = !v;
    fire(document, new BaseEvent('visibilitychange', { bubbles: true }));
    fire(global, new PageTransitionEvent(v ? 'pageshow' : 'pagehide', { persisted: false }));
  };

  function answer(m, ok) { if (m.seq) send('A' + m.seq + ' ' + (ok ? 1 : 0)); }

  // Messages are tasks, in the order they came; the browser numbers its inputs (`n`).
  const baseOnMessage = kjs.onMessage;
  kjs.onMessage = function (text) {
    const t = /^\{"t":"(\w+)"/.exec(text.slice(0, 16));
    const h = t && handlers[t[1]];
    if (!h) {
      baseOnMessage(text);
      if (t && t[1] === 'init') D.tasks.push({ name: 'input', run: initDone });
      return;
    }
    const m = JSON.parse(text);
    D.tasks.push({ name: m.t, run() { try { h(m); } finally { if (m.n > ack) ack = m.n; } } });
  };
  function initDone() {
    scanHandlers(document);
    // `autofocus`: when the page is ready, the first such control takes the focus.
    D.tasks.push({ name: 'autofocus', run() {} });
    global.addEventListener('load', () => {
      if (document._active) return;
      const a = D.descendants(document).find((e) => e.hasAttribute('autofocus') && focusable(e));
      if (a) a.focus();
    }, { once: true });
  }
  // The document sent after an input reflects it: say up to which one first.
  const baseFlush = kjs.flush;
  kjs.flush = function () {
    if (ack !== ackSent && D.isDirty()) { send('Q' + ack); ackSent = ack; }
    baseFlush();
  };
})(globalThis, globalThis.__kjs_native, globalThis.__kjs);
