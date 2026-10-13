// Kitsune jsrun --page: a small `Intl`.
//
// QuickJS has no `Intl`; pages and libraries reach for it to show dates and numbers. This is the part
// they use, in Portuguese (pt-BR, the default) and English (en, en-US): `DateTimeFormat`,
// `NumberFormat` (decimal, percent, currency, compact), `PluralRules`, `Collator`,
// `RelativeTimeFormat`, `ListFormat`, `Segmenter` (graphemes and words, roughly) and the
// `toLocale*String` and `localeCompare` methods that use them. Time zones: UTC (the system clock is
// UTC) and the fixed-offset zones of Brazil; any other zone is shown as UTC.
(function (global) {
  'use strict';
  if (typeof global.Intl === 'object' && global.Intl && global.Intl.DateTimeFormat) return;

  const pickLocale = (locales) => {
    const list = [].concat(locales === undefined || locales === null ? [] : locales).map(String);
    for (const l of list) {
      const low = l.toLowerCase();
      if (low.startsWith('pt')) return 'pt-BR';
      if (low.startsWith('en')) return low === 'en-gb' ? 'en-GB' : 'en-US';
    }
    const nav = global.navigator && global.navigator.language ? String(global.navigator.language).toLowerCase() : 'pt-br';
    return nav.startsWith('en') ? 'en-US' : 'pt-BR';
  };
  const isPt = (l) => l === 'pt-BR';

  // ---- numbers ----

  function group(int, sep) {
    let out = '';
    for (let i = 0; i < int.length; i++) {
      if (i && (int.length - i) % 3 === 0) out += sep;
      out += int[i];
    }
    return out;
  }
  const CURRENCY = { BRL: 'R$', USD: '$', EUR: '€', GBP: '£', JPY: '¥' };
  class NumberFormat {
    constructor(locales, o) {
      o = o || {};
      this._l = pickLocale(locales);
      this._style = o.style || 'decimal';
      this._cur = o.currency ? String(o.currency).toUpperCase() : null;
      if (this._style === 'currency' && !this._cur) throw new TypeError('Currency code is required with currency style.');
      const curDigits = this._cur === 'JPY' ? 0 : 2;
      const dMin = this._style === 'currency' ? curDigits : 0;
      const dMax = this._style === 'currency' ? curDigits : this._style === 'percent' ? 0 : 3;
      this._min = o.minimumFractionDigits ?? Math.min(dMin, o.maximumFractionDigits ?? dMin);
      this._max = Math.max(this._min, o.maximumFractionDigits ?? Math.max(dMax, this._min));
      this._minInt = o.minimumIntegerDigits || 1;
      this._sig = o.maximumSignificantDigits || 0;
      this._group = o.useGrouping === undefined ? true : !!o.useGrouping && o.useGrouping !== 'false';
      this._notation = o.notation || 'standard';
      this._display = o.currencyDisplay || 'symbol';
      this._sign = o.signDisplay || 'auto';
      this._unit = o.unit || null;
      this._unitDisplay = o.unitDisplay || 'short';
    }
    format(n) { return this.formatToParts(n).map((p) => p.value).join(''); }
    formatToParts(x) {
      let n = typeof x === 'bigint' ? Number(x) : Number(x);
      const pt = isPt(this._l);
      const dec = pt ? ',' : '.', grp = pt ? '.' : ',';
      const parts = [];
      if (Number.isNaN(n)) return [{ type: 'nan', value: 'NaN' }];
      const neg = n < 0 || Object.is(n, -0);
      n = Math.abs(n);
      if (this._style === 'percent') n *= 100;
      let suffix = '';
      let max = this._max, min = this._min;
      if (this._notation === 'compact') {
        const steps = pt ? [[1e12, ' tri'], [1e9, ' bi'], [1e6, ' mi'], [1e3, ' mil']] : [[1e12, 'T'], [1e9, 'B'], [1e6, 'M'], [1e3, 'K']];
        for (const [v, s] of steps) if (n >= v) { n /= v; suffix = s; break; }
        max = n < 100 && suffix ? 1 : 0; min = 0;
      }
      if (!Number.isFinite(n)) parts.push({ type: 'infinity', value: '∞' });
      else {
        let text;
        if (this._sig) text = String(Number(n.toPrecision(this._sig)));
        else {
          text = n.toFixed(max);
          if (max > min) text = text.replace(/0+$/, (z) => z.slice(0, Math.max(0, z.length - (max - min)))).replace(/\.$/, '');
          if (text.includes('.')) { const [i, f] = text.split('.'); if (f.length < min) text = i + '.' + f.padEnd(min, '0'); }
          else if (min > 0) text += '.' + '0'.repeat(min);
        }
        let [int, frac] = text.split('.');
        int = int.padStart(this._minInt, '0');
        parts.push({ type: 'integer', value: this._group ? group(int, grp) : int });
        if (frac) { parts.push({ type: 'decimal', value: dec }); parts.push({ type: 'fraction', value: frac }); }
      }
      if (suffix) parts.push({ type: 'compact', value: suffix });
      if (this._style === 'percent') parts.push({ type: 'percentSign', value: '%' });
      if (this._style === 'unit' && this._unit) parts.push({ type: 'literal', value: ' ' }, { type: 'unit', value: this._unit });
      const isZero = parts.every((p) => p.type !== 'integer' || /^0+$/.test(p.value)) && !parts.some((p) => p.type === 'fraction' && /[1-9]/.test(p.value));
      let sign = '';
      if (neg && !(isZero && this._sign === 'auto')) sign = '-';
      else if (!neg && (this._sign === 'always' || (this._sign === 'exceptZero' && !isZero))) sign = '+';
      if (this._style === 'currency') {
        const sym = this._display === 'code' ? this._cur : this._display === 'name' ? this._cur : CURRENCY[this._cur] || this._cur;
        const cur = { type: 'currency', value: sym };
        if (pt) parts.unshift(cur, { type: 'literal', value: ' ' });
        else parts.unshift(cur);
      }
      if (sign) parts.unshift({ type: sign === '-' ? 'minusSign' : 'plusSign', value: sign });
      return parts;
    }
    formatRange(a, b) { return this.format(a) + '–' + this.format(b); }
    resolvedOptions() {
      return { locale: this._l, numberingSystem: 'latn', style: this._style, currency: this._cur || undefined,
        minimumIntegerDigits: this._minInt, minimumFractionDigits: this._min, maximumFractionDigits: this._max,
        useGrouping: this._group ? 'auto' : false, notation: this._notation, signDisplay: this._sign };
    }
    static supportedLocalesOf(l) { return [].concat(l || []).filter((x) => /^(pt|en)/i.test(x)); }
  }

  // ---- dates ----

  const MONTHS = {
    pt: ['janeiro', 'fevereiro', 'março', 'abril', 'maio', 'junho', 'julho', 'agosto', 'setembro', 'outubro', 'novembro', 'dezembro'],
    en: ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'],
  };
  const DAYS = {
    pt: ['domingo', 'segunda-feira', 'terça-feira', 'quarta-feira', 'quinta-feira', 'sexta-feira', 'sábado'],
    en: ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'],
  };
  const ZONES = { UTC: 0, 'Etc/UTC': 0, GMT: 0, 'America/Sao_Paulo': -180, 'America/Fortaleza': -180, 'America/Recife': -180,
    'America/Bahia': -180, 'America/Belem': -180, 'America/Manaus': -240, 'America/Cuiaba': -240, 'America/Porto_Velho': -240,
    'America/Boa_Vista': -240, 'America/Rio_Branco': -300, 'America/Noronha': -120 };
  const two = (n) => String(n).padStart(2, '0');
  class DateTimeFormat {
    constructor(locales, o) {
      o = Object.assign({}, o);
      this._l = pickLocale(locales);
      this._tz = o.timeZone && ZONES[o.timeZone] !== undefined ? o.timeZone : 'UTC';
      const dateKeys = ['weekday', 'era', 'year', 'month', 'day'];
      const timeKeys = ['hour', 'minute', 'second', 'fractionalSecondDigits', 'dayPeriod'];
      const any = dateKeys.concat(timeKeys).some((k) => o[k] !== undefined) || o.dateStyle || o.timeStyle || o.timeZoneName;
      if (o.dateStyle) Object.assign(o, { full: { weekday: 'long', year: 'numeric', month: 'long', day: 'numeric' },
        long: { year: 'numeric', month: 'long', day: 'numeric' }, medium: { year: 'numeric', month: 'short', day: 'numeric' },
        short: { year: isPt(this._l) ? 'numeric' : '2-digit', month: '2-digit', day: '2-digit' } }[o.dateStyle] || {});
      if (o.timeStyle) Object.assign(o, { hour: '2-digit', minute: '2-digit' }, o.timeStyle === 'short' ? {} : { second: '2-digit' });
      if (!any) Object.assign(o, { year: 'numeric', month: 'numeric', day: 'numeric' });
      this._o = o;
      this._h12 = o.hour12 !== undefined ? !!o.hour12 : o.hourCycle ? /h12|h11/.test(o.hourCycle) : !isPt(this._l) && this._l !== 'en-GB';
    }
    formatToParts(d) {
      const t = d === undefined ? Date.now() : +(d instanceof Date ? d : new Date(d));
      if (!Number.isFinite(t)) throw new RangeError('Invalid time value');
      const x = new Date(t + ZONES[this._tz] * 60000);
      const o = this._o;
      const pt = isPt(this._l);
      const lang = pt ? 'pt' : 'en';
      const Y = x.getUTCFullYear(), M = x.getUTCMonth(), Dd = x.getUTCDate(), W = x.getUTCDay();
      const h = x.getUTCHours(), mi = x.getUTCMinutes(), s = x.getUTCSeconds();
      const v = {};
      if (o.weekday) v.weekday = o.weekday === 'long' ? DAYS[lang][W] : o.weekday === 'short' ? (pt ? DAYS.pt[W].slice(0, 3) + '.' : DAYS.en[W].slice(0, 3)) : DAYS[lang][W][0].toUpperCase();
      if (o.year) v.year = o.year === '2-digit' ? two(Y % 100) : String(Y);
      if (o.month) {
        if (o.month === 'long') v.month = MONTHS[lang][M];
        else if (o.month === 'short') v.month = pt ? MONTHS.pt[M].slice(0, 3) + '.' : MONTHS.en[M].slice(0, 3);
        else if (o.month === 'narrow') v.month = MONTHS[lang][M][0].toUpperCase();
        else v.month = o.month === '2-digit' || (pt && (o.day || o.year)) ? two(M + 1) : String(M + 1);
      }
      if (o.day) v.day = o.day === '2-digit' || (pt && o.month && !/long|short|narrow/.test(o.month)) ? two(Dd) : String(Dd);
      if (o.hour !== undefined) {
        let hh = h;
        if (this._h12) { v.dayPeriod = h < 12 ? 'AM' : 'PM'; hh = h % 12 || 12; }
        v.hour = o.hour === '2-digit' || (!this._h12 && o.minute) ? two(hh) : String(hh);
      }
      if (o.minute !== undefined) v.minute = two(mi);
      if (o.second !== undefined) v.second = two(s);
      if (o.fractionalSecondDigits) v.fractionalSecond = String(x.getUTCMilliseconds()).padStart(3, '0').slice(0, o.fractionalSecondDigits);
      if (o.timeZoneName) v.timeZoneName = this._tz === 'UTC' ? 'UTC' : 'GMT' + (ZONES[this._tz] / 60);
      const parts = [];
      const add = (type, value) => { if (value !== undefined) parts.push({ type, value }); };
      const lit = (value) => parts.push({ type: 'literal', value });
      const textMonth = o.month && /long|short|narrow/.test(o.month);
      // The date.
      if (v.weekday) { add('weekday', v.weekday); if (v.day || v.month || v.year) lit(pt ? ', ' : ', '); }
      if (textMonth) {
        if (pt) {
          if (v.day) { add('day', v.day); lit(' de '); }
          add('month', v.month);
          if (v.year) { lit(' de '); add('year', v.year); }
        } else {
          add('month', v.month);
          if (v.day) { lit(' '); add('day', v.day); }
          if (v.year) { lit(v.day ? ', ' : ' '); add('year', v.year); }
        }
      } else {
        const order = pt || this._l === 'en-GB' ? ['day', 'month', 'year'] : ['month', 'day', 'year'];
        let first = true;
        for (const k of order) if (v[k]) { if (!first) lit('/'); add(k, v[k]); first = false; }
      }
      // The time.
      if (v.hour !== undefined || v.minute !== undefined) {
        if (parts.length) lit(pt ? ' ' : ', ');
        if (v.hour !== undefined) add('hour', v.hour);
        if (v.minute !== undefined) { if (v.hour !== undefined) lit(':'); add('minute', v.minute); }
        if (v.second !== undefined) { lit(':'); add('second', v.second); }
        if (v.fractionalSecond) { lit(pt ? ',' : '.'); add('fractionalSecond', v.fractionalSecond); }
        if (v.dayPeriod) { lit(' '); add('dayPeriod', v.dayPeriod); }
      }
      if (v.timeZoneName) { lit(' '); add('timeZoneName', v.timeZoneName); }
      return parts;
    }
    format(d) { return this.formatToParts(d).map((p) => p.value).join(''); }
    formatRange(a, b) { return this.format(a) + ' – ' + this.format(b); }
    resolvedOptions() {
      return Object.assign({ locale: this._l, calendar: 'gregory', numberingSystem: 'latn', timeZone: this._tz, hour12: this._h12 }, this._o);
    }
    static supportedLocalesOf(l) { return NumberFormat.supportedLocalesOf(l); }
  }

  // ---- words ----

  class PluralRules {
    constructor(locales, o) { this._l = pickLocale(locales); this._type = (o && o.type) || 'cardinal'; }
    select(n) {
      n = Number(n);
      if (this._type === 'ordinal') {
        if (isPt(this._l)) return 'other';
        const t = n % 10, c = n % 100;
        return t === 1 && c !== 11 ? 'one' : t === 2 && c !== 12 ? 'two' : t === 3 && c !== 13 ? 'few' : 'other';
      }
      if (isPt(this._l)) return Math.floor(Math.abs(n)) <= 1 ? 'one' : 'other';
      return n === 1 ? 'one' : 'other';
    }
    resolvedOptions() { return { locale: this._l, type: this._type, pluralCategories: isPt(this._l) ? ['one', 'other'] : ['one', 'other'] }; }
    static supportedLocalesOf(l) { return NumberFormat.supportedLocalesOf(l); }
  }
  const fold = (s, accents) => {
    s = String(s);
    if (!accents) s = s.normalize('NFD').replace(/[̀-ͯ]/g, '');
    return s;
  };
  class Collator {
    constructor(locales, o) {
      o = o || {};
      this._l = pickLocale(locales);
      this._s = o.sensitivity || 'variant';
      this._num = !!o.numeric;
      this._ignore = !!o.ignorePunctuation;
      this.compare = this.compare.bind(this);
    }
    compare(a, b) {
      let x = String(a), y = String(b);
      if (this._ignore) { x = x.replace(/[\p{P}\s]/gu, ''); y = y.replace(/[\p{P}\s]/gu, ''); }
      const base = (s) => fold(s, false).toLowerCase();
      const cmp = (p, q) => {
        if (this._num) {
          const re = /(\d+)|(\D+)/g;
          const pa = p.match(re) || [], qa = q.match(re) || [];
          for (let i = 0; i < Math.min(pa.length, qa.length); i++) {
            const u = pa[i], v = qa[i];
            if (/^\d/.test(u) && /^\d/.test(v)) { const d = Number(u) - Number(v); if (d) return d < 0 ? -1 : 1; }
            else if (u !== v) return u < v ? -1 : 1;
          }
          return pa.length === qa.length ? 0 : pa.length < qa.length ? -1 : 1;
        }
        return p === q ? 0 : p < q ? -1 : 1;
      };
      const r = cmp(base(x), base(y));
      if (r || this._s === 'base') return r;
      if (this._s === 'accent') return cmp(x.toLowerCase(), y.toLowerCase());
      const r2 = cmp(fold(x, false), fold(y, false));
      if (r2 && this._s !== 'accent') {
        // Lower case first, as in browsers.
        const lx = fold(x, false), ly = fold(y, false);
        if (lx.toLowerCase() === ly.toLowerCase()) return lx < ly ? 1 : -1;
        return r2;
      }
      return cmp(x, y);
    }
    resolvedOptions() { return { locale: this._l, usage: 'sort', sensitivity: this._s, numeric: this._num, ignorePunctuation: this._ignore, collation: 'default', caseFirst: 'false' }; }
    static supportedLocalesOf(l) { return NumberFormat.supportedLocalesOf(l); }
  }
  const UNITS = {
    pt: { second: ['segundo', 'segundos'], minute: ['minuto', 'minutos'], hour: ['hora', 'horas'], day: ['dia', 'dias'], week: ['semana', 'semanas'], month: ['mês', 'meses'], quarter: ['trimestre', 'trimestres'], year: ['ano', 'anos'] },
    en: { second: ['second', 'seconds'], minute: ['minute', 'minutes'], hour: ['hour', 'hours'], day: ['day', 'days'], week: ['week', 'weeks'], month: ['month', 'months'], quarter: ['quarter', 'quarters'], year: ['year', 'years'] },
  };
  class RelativeTimeFormat {
    constructor(locales, o) { this._l = pickLocale(locales); this._numeric = (o && o.numeric) || 'always'; }
    format(v, unit) {
      v = Number(v);
      unit = String(unit).replace(/s$/, '');
      const pt = isPt(this._l);
      const words = (pt ? UNITS.pt : UNITS.en)[unit];
      if (!words) throw new RangeError('Invalid unit argument for format() \'' + unit + '\'');
      if (this._numeric === 'auto') {
        const auto = pt ? { day: { '-1': 'ontem', 0: 'hoje', 1: 'amanhã' }, year: { '-1': 'ano passado', 0: 'este ano', 1: 'próximo ano' }, second: { 0: 'agora' } }
          : { day: { '-1': 'yesterday', 0: 'today', 1: 'tomorrow' }, year: { '-1': 'last year', 0: 'this year', 1: 'next year' }, second: { 0: 'now' } };
        const a = auto[unit] && auto[unit][String(v)];
        if (a) return a;
      }
      const n = Math.abs(v);
      const w = n === 1 ? words[0] : words[1];
      const num = new NumberFormat(this._l).format(n);
      if (pt) return v < 0 || Object.is(v, -0) ? 'há ' + num + ' ' + w : 'em ' + num + ' ' + w;
      return v < 0 || Object.is(v, -0) ? num + ' ' + w + ' ago' : 'in ' + num + ' ' + w;
    }
    formatToParts(v, u) { return [{ type: 'literal', value: this.format(v, u) }]; }
    resolvedOptions() { return { locale: this._l, style: 'long', numeric: this._numeric, numberingSystem: 'latn' }; }
  }
  class ListFormat {
    constructor(locales, o) { this._l = pickLocale(locales); this._type = (o && o.type) || 'conjunction'; }
    format(list) {
      const a = [...list].map(String);
      const pt = isPt(this._l);
      const word = this._type === 'disjunction' ? (pt ? ' ou ' : ' or ') : this._type === 'unit' ? ', ' : pt ? ' e ' : ' and ';
      if (a.length < 2) return a.join('');
      if (a.length === 2) return a[0] + word + a[1];
      return a.slice(0, -1).join(', ') + (pt || this._type === 'unit' ? word : ',' + word) + a[a.length - 1];
    }
    formatToParts(list) { return [{ type: 'literal', value: this.format(list) }]; }
  }
  class Segmenter {
    constructor(locales, o) { this._g = (o && o.granularity) || 'grapheme'; }
    segment(s) {
      s = String(s);
      const out = [];
      if (this._g === 'word') {
        const re = /[\p{L}\p{N}_']+|[^\p{L}\p{N}_']+/gu;
        let m;
        while ((m = re.exec(s))) out.push({ segment: m[0], index: m.index, input: s, isWordLike: /[\p{L}\p{N}]/u.test(m[0]) });
      } else if (this._g === 'sentence') {
        const re = /[^.!?]+[.!?]*\s*/g;
        let m;
        while ((m = re.exec(s))) out.push({ segment: m[0], index: m.index, input: s });
      } else {
        let i = 0;
        for (const ch of s.match(/\P{M}\p{M}*/gu) || []) { out.push({ segment: ch, index: i, input: s }); i += ch.length; }
      }
      out.containing = (i) => out.find((x) => i >= x.index && i < x.index + x.segment.length);
      return out;
    }
  }
  class DisplayNames {
    constructor(locales, o) { this._l = pickLocale(locales); this._type = o && o.type; }
    of(code) {
      const pt = isPt(this._l);
      const names = { language: { pt: pt ? 'português' : 'Portuguese', en: pt ? 'inglês' : 'English', es: pt ? 'espanhol' : 'Spanish' },
        region: { BR: pt ? 'Brasil' : 'Brazil', US: pt ? 'Estados Unidos' : 'United States', PT: 'Portugal' },
        currency: { BRL: pt ? 'Real brasileiro' : 'Brazilian Real', USD: pt ? 'Dólar americano' : 'US Dollar' } }[this._type] || {};
      return names[code] || names[String(code).split('-')[0]] || code;
    }
  }
  class Locale {
    constructor(tag) {
      const [language, region] = String(tag).split(/[-_]/);
      this.language = language.toLowerCase(); this.region = region ? region.toUpperCase() : undefined;
      this.baseName = this.language + (this.region ? '-' + this.region : '');
    }
    toString() { return this.baseName; }
    maximize() { return this; }
    minimize() { return this; }
  }
  const Intl = {
    DateTimeFormat, NumberFormat, PluralRules, Collator, RelativeTimeFormat, ListFormat, Segmenter, DisplayNames, Locale,
    getCanonicalLocales(l) { return [].concat(l || []).map((x) => { const [a, b] = String(x).split(/[-_]/); return a.toLowerCase() + (b ? '-' + b.toUpperCase() : ''); }); },
    supportedValuesOf(k) { return k === 'timeZone' ? Object.keys(ZONES) : k === 'currency' ? Object.keys(CURRENCY) : []; },
  };
  // Calls without `new` work for the old ones, as in browsers.
  for (const k of ['DateTimeFormat', 'NumberFormat', 'Collator']) {
    const C = Intl[k];
    const F = function (l, o) { return new C(l, o); };
    F.prototype = C.prototype;
    F.supportedLocalesOf = C.supportedLocalesOf;
    Intl[k] = F;
  }
  Object.defineProperty(global, 'Intl', { value: Intl, writable: true, configurable: true, enumerable: false });

  const def = (proto, name, fn) => Object.defineProperty(proto, name, { value: fn, writable: true, configurable: true, enumerable: false });
  def(Number.prototype, 'toLocaleString', function (l, o) { return new NumberFormat(l, o).format(this.valueOf()); });
  if (typeof BigInt === 'function') def(BigInt.prototype, 'toLocaleString', function (l, o) { return new NumberFormat(l, o).format(this.valueOf()); });
  def(Date.prototype, 'toLocaleDateString', function (l, o) {
    o = Object.assign({}, o);
    if (!['weekday', 'year', 'month', 'day', 'dateStyle'].some((k) => o[k])) Object.assign(o, { year: 'numeric', month: 'numeric', day: 'numeric' });
    return new DateTimeFormat(l, o).format(this);
  });
  def(Date.prototype, 'toLocaleTimeString', function (l, o) {
    o = Object.assign({}, o);
    if (!['hour', 'minute', 'second', 'timeStyle'].some((k) => o[k])) Object.assign(o, { hour: 'numeric', minute: '2-digit', second: '2-digit' });
    return new DateTimeFormat(l, o).format(this);
  });
  def(Date.prototype, 'toLocaleString', function (l, o) {
    o = Object.assign({}, o);
    if (!['weekday', 'year', 'month', 'day', 'hour', 'minute', 'second', 'dateStyle', 'timeStyle'].some((k) => o[k])) {
      Object.assign(o, { year: 'numeric', month: 'numeric', day: 'numeric', hour: 'numeric', minute: '2-digit', second: '2-digit' });
    }
    return new DateTimeFormat(l, o).format(this);
  });
  def(String.prototype, 'localeCompare', function (that, l, o) { return new Collator(l, o).compare(String(this), String(that)); });
  def(Array.prototype, 'toLocaleString', function (l, o) { return this.map((x) => (x === null || x === undefined ? '' : x.toLocaleString(l, o))).join(','); });
})(globalThis);
