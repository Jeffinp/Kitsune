// Kitsune jsrun --page: the state of the form controls.
//
// Evaluated after dom.js and net.js, before events.js. A control's state lives here, as in a browser:
// `value` (the attribute is only the default: `defaultValue`), `checked` (`defaultChecked`),
// `selectedIndex` and `option.selected`, `type`, `form`, `form.elements`, `labels`/`control`,
// validation (`validity`, `checkValidity`...), the selection range of a text field. The browser's copy of
// the document carries the live state (the `wireAttrs`/`wireKids` hooks of dom.js): what a script sets
// is what shows on screen, and the text the person types comes back from the browser (events.js).
// The helpers the other files use are `kjs.forms`.
(function (global, native, kjs) {
  'use strict';
  const D = kjs.dom;
  const { document, dispatch, touch, Element, Node } = D;

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
    select() { this._kjsSelStart = 0; this._kjsSelEnd = (this.value || '').length; dispatch(this, new D.Event('select', { bubbles: true })); },
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

  kjs.forms = { isInput, inputType, toggles, isControl, textLike, optionsOf, selectOf, formControls };
})(globalThis, globalThis.__kjs_native, globalThis.__kjs);
