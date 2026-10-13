// Kitsune jsrun --page: <video> and <audio> (HTMLMediaElement) and Media Source Extensions.
//
// Evaluated after dom.js and net.js (same prelude). The playing happens in the browser: each element
// that loads gets a player there (and a decoder process, `mediadec --serve`); this side is the API.
// Commands go to the browser as `M` messages (`open`, `play`, `pause`, `seek`, `vol`, `eos`...), the
// bytes of the media as `m` messages (`m<id> <source> <seq>\n` + WebM bytes), and the browser answers
// with JSON: the player's state and the events to fire, what each source buffer holds after an
// append, where a cluster is. See kitsune_core::media::wire and docs/design/tls-browser.md §14.
//
// A `src` that is an http(s) address is fetched by the browser itself, straight into the decoder (the
// page never reads those bytes, so another origin plays without CORS); a `blob:` URL of a MediaSource
// attaches it; a `blob:` of a Blob or a `data:` address is read here and appended to source 0.
(function (global, native, kjs) {
  'use strict';
  const send = native.send;
  const sendBytes = native.sendBytes;
  const NS = 1e9;
  const now = () => performance.now();
  const report = (e) => console.error('Uncaught ' + String(e) + (e && e.stack ? '\n' + e.stack : ''));
  const define = (name, value) => {
    Object.defineProperty(global, name, { value, writable: true, configurable: true, enumerable: false });
  };
  const later = (fn) => setTimeout(fn, 0);
  const ns = (s) => Math.round(Number(s) * NS);

  // ---- TimeRanges and MediaError ----

  class TimeRanges {
    constructor(list) { this._r = list || []; }
    get length() { return this._r.length; }
    start(i) {
      if (!(i >= 0 && i < this._r.length)) throw new DOMException('index ' + i + ' out of range', 'IndexSizeError');
      return this._r[i][0];
    }
    end(i) {
      if (!(i >= 0 && i < this._r.length)) throw new DOMException('index ' + i + ' out of range', 'IndexSizeError');
      return this._r[i][1];
    }
  }
  // Ranges in seconds from the browser's nanoseconds.
  const fromNs = (list) => (list || []).map(([a, b]) => [a / NS, b / NS]);
  // The times in every list (each sorted and disjoint).
  function intersect(lists) {
    if (!lists.length) return [];
    let out = lists[0];
    for (const l of lists.slice(1)) {
      const r = [];
      let i = 0, j = 0;
      while (i < out.length && j < l.length) {
        const s = Math.max(out[i][0], l[j][0]), e = Math.min(out[i][1], l[j][1]);
        if (s < e) r.push([s, e]);
        if (out[i][1] < l[j][1]) i++; else j++;
      }
      out = r;
    }
    return out;
  }
  // Add [a, b] to a sorted, disjoint list.
  function addRange(list, a, b) {
    if (!(b > a)) return list;
    const out = [];
    let placed = false;
    for (const [s, e] of list) {
      if (e < a || placed) { out.push([s, e]); continue; }
      if (s > b) { out.push([a, b], [s, e]); placed = true; continue; }
      a = Math.min(a, s); b = Math.max(b, e);
    }
    if (!placed) out.push([a, b]);
    return out.sort((x, y) => x[0] - y[0]);
  }

  class MediaError {
    constructor(code, message) { this.code = code; this.message = message || ''; }
  }
  Object.assign(MediaError, { MEDIA_ERR_ABORTED: 1, MEDIA_ERR_NETWORK: 2, MEDIA_ERR_DECODE: 3, MEDIA_ERR_SRC_NOT_SUPPORTED: 4 });
  Object.assign(MediaError.prototype, { MEDIA_ERR_ABORTED: 1, MEDIA_ERR_NETWORK: 2, MEDIA_ERR_DECODE: 3, MEDIA_ERR_SRC_NOT_SUPPORTED: 4 });

  // ---- what can be played: WebM with VP9 and Opus ----

  function parseType(type) {
    const m = /^\s*([a-z]+)\/([-+.\w]+)\s*(;.*)?$/i.exec(String(type));
    if (!m) return null;
    const codecs = /codecs\s*=\s*(?:"([^"]*)"|'([^']*)'|([^;]*))/i.exec(m[3] || '');
    const list = codecs ? (codecs[1] ?? codecs[2] ?? codecs[3] ?? '').split(',').map((c) => c.trim().toLowerCase()).filter(Boolean) : null;
    return { major: m[1].toLowerCase(), sub: m[2].toLowerCase(), codecs: list };
  }
  const codecOk = (c, major) => c === 'opus' || (major === 'video' && (c === 'vp9' || c === 'vp9.0' || /^vp09(\.\d+)*$/.test(c)));
  // '', 'maybe' or 'probably' (canPlayType).
  function support(type) {
    const t = parseType(type);
    if (!t || t.sub !== 'webm' || (t.major !== 'video' && t.major !== 'audio')) return '';
    if (!t.codecs) return 'maybe';
    if (!t.codecs.length || !t.codecs.every((c) => codecOk(c, t.major))) return '';
    // VP9 profile 0 (8 bits) only: vp09.00.*.08.
    for (const c of t.codecs) {
      const v = /^vp09\.(\d+)(?:\.\d+)?(?:\.(\d+))?/.exec(c);
      if (v && (Number(v[1]) !== 0 || (v[2] !== undefined && Number(v[2]) !== 8))) return '';
    }
    // `width`, `height` and `framerate` (the parameters YouTube's player probes, as TV browsers
    // honour them): what the software decoder keeps up with in real time even under emulation
    // (VP9 480p30: ~25 fps decoded under QEMU without KVM, docs/design/midia.md), so a player that
    // asks picks a stream that plays. Other parameters are ignored (as Chrome does).
    const num = (k) => { const m = new RegExp(';\\s*' + k + '\\s*=\\s*"?(\\d+(?:\\.\\d+)?)', 'i').exec(String(type)); return m ? Number(m[1]) : 0; };
    if (t.major === 'video') {
      const w = num('width'), h = num('height'), fps = num('framerate');
      const fits = (w <= MAX_VIDEO_W && h <= MAX_VIDEO_H) || (w <= MAX_VIDEO_H && h <= MAX_VIDEO_W);
      if (!fits || fps > MAX_VIDEO_FPS) return '';
    }
    return 'probably';
  }
  const MAX_VIDEO_W = 854, MAX_VIDEO_H = 480, MAX_VIDEO_FPS = 30;

  // ---- event targets with on<type> properties ----

  const ETarget = global.EventTarget;
  function handlerProps(proto, types) {
    for (const type of types) {
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
  function fire(target, type) {
    try { target.dispatchEvent(new Event(type)); } catch (e) { report(e); }
  }

  // ---- the elements ----

  const MEDIA_EVENTS = ['loadstart', 'progress', 'suspend', 'abort', 'error', 'emptied', 'stalled',
    'loadedmetadata', 'loadeddata', 'canplay', 'canplaythrough', 'playing', 'waiting', 'seeking',
    'seeked', 'ended', 'durationchange', 'timeupdate', 'play', 'pause', 'ratechange', 'resize',
    'volumechange'];
  const players = new Map();
  const msUrls = new Map();

  function state(el) {
    return el._media || (el._media = {
      open: false, paused: true, ended: false, seeking: false, ct: 0, ctAt: 0, lastCt: 0,
      duration: NaN, readyState: 0, networkState: 0, vw: 0, vh: 0, error: null,
      volume: 1, muted: null, rate: 1, defaultRate: 1, currentSrc: '', source: null, loader: null,
      playPromises: [], played: [], startAt: 0, autoplayed: false, ready: 0,
    });
  }

  class HTMLMediaElement extends HTMLElement {
    // ---- attributes ----
    get src() {
      const v = this.getAttribute('src');
      if (v === null) return '';
      if (/^(blob|data):/i.test(v.trim())) return v.trim();
      try { return new URL(v, location.href).href; } catch (e) { return v; }
    }
    set src(v) { this.setAttribute('src', v); }
    setAttribute(n, v) {
      super.setAttribute(n, v);
      const k = String(n).toLowerCase();
      if (k === 'src') this.load();
      else if (k === 'loop') this._cmd('loop', 1);
    }
    removeAttribute(n) {
      super.removeAttribute(n);
      const k = String(n).toLowerCase();
      if (k === 'src') this.load();
      else if (k === 'loop') this._cmd('loop', 0);
    }
    get currentSrc() { return state(this).currentSrc; }
    get srcObject() { return state(this).srcObject || null; }
    set srcObject(o) { state(this).srcObject = o || null; this.load(); }
    get crossOrigin() { return this.getAttribute('crossorigin'); }
    set crossOrigin(v) { if (v === null) this.removeAttribute('crossorigin'); else this.setAttribute('crossorigin', v); }
    get preload() { return (this.getAttribute('preload') || 'metadata').toLowerCase(); }
    set preload(v) { this.setAttribute('preload', v); }
    get autoplay() { return this.hasAttribute('autoplay'); }
    set autoplay(v) { this.toggleAttribute('autoplay', !!v); }
    get loop() { return this.hasAttribute('loop'); }
    set loop(v) { this.toggleAttribute('loop', !!v); }
    get controls() { return this.hasAttribute('controls'); }
    set controls(v) { this.toggleAttribute('controls', !!v); }
    get defaultMuted() { return this.hasAttribute('muted'); }
    set defaultMuted(v) { this.toggleAttribute('muted', !!v); }
    get playsInline() { return this.hasAttribute('playsinline'); }
    set playsInline(v) { this.toggleAttribute('playsinline', !!v); }

    // ---- state ----
    get networkState() { return state(this).networkState; }
    get readyState() { return state(this).readyState; }
    get error() { return state(this).error; }
    get paused() { return state(this).paused; }
    get ended() { return state(this).ended; }
    get seeking() { return state(this).seeking; }
    get videoWidth() { return state(this).vw; }
    get videoHeight() { return state(this).vh; }
    get duration() {
      const s = state(this);
      if (s.source && s.source.readyState !== 'closed') return s.readyState ? s.source.duration : NaN;
      return s.duration;
    }
    get currentTime() {
      const s = state(this);
      if (s.seeking || s.paused || s.ended || s.readyState < 3) return s.ct;
      // Between two updates from the browser the time runs on (never backwards).
      const d = this.duration;
      let t = s.ct + Math.min((now() - s.ctAt) / 1000, 0.5);
      if (d >= 0) t = Math.min(t, d);
      t = Math.max(t, s.lastCt);
      s.lastCt = t;
      return t;
    }
    set currentTime(v) {
      const t = Number(v);
      if (!isFinite(t)) throw new TypeError('currentTime must be a finite number');
      const s = state(this);
      if (s.readyState === 0) { s.startAt = Math.max(0, t); return; }
      this._seek(Math.max(0, t));
    }
    fastSeek(t) { this.currentTime = t; }
    get defaultPlaybackRate() { return state(this).defaultRate; }
    set defaultPlaybackRate(v) { state(this).defaultRate = Number(v) || 1; }
    get playbackRate() { return state(this).rate; }
    set playbackRate(v) {
      // Only normal speed plays; the value is kept so pages that set it back see it.
      const s = state(this);
      const r = Number(v);
      if (!isFinite(r)) throw new TypeError('playbackRate must be finite');
      if (r !== s.rate) { s.rate = r; later(() => fire(this, 'ratechange')); }
    }
    get preservesPitch() { return true; }
    set preservesPitch(v) {}
    get volume() { return state(this).volume; }
    set volume(v) {
      const x = Number(v);
      if (!(x >= 0 && x <= 1)) throw new DOMException('volume must be between 0 and 1', 'IndexSizeError');
      const s = state(this);
      if (x === s.volume) return;
      s.volume = x;
      this._sendVolume();
      later(() => fire(this, 'volumechange'));
    }
    get muted() { const s = state(this); return s.muted === null ? this.defaultMuted : s.muted; }
    set muted(v) {
      const s = state(this);
      const m = !!v;
      if (m === this.muted) { s.muted = m; return; }
      s.muted = m;
      this._sendVolume();
      later(() => fire(this, 'volumechange'));
    }
    get buffered() {
      const s = state(this);
      if (s.source) return new TimeRanges(s.source._buffered());
      if (s.loader) return new TimeRanges(s.loader.ranges.slice());
      return new TimeRanges([]);
    }
    get seekable() {
      const d = this.duration;
      if (d > 0 && isFinite(d)) return new TimeRanges([[0, d]]);
      if (d === Infinity) return this.buffered;
      return new TimeRanges([]);
    }
    get played() { return new TimeRanges(state(this).played.slice()); }
    get textTracks() { return Object.assign([], { addEventListener() {}, removeEventListener() {}, getTrackById: () => null }); }
    get audioTracks() { return Object.assign([], { addEventListener() {}, removeEventListener() {}, getTrackById: () => null }); }
    get videoTracks() { return Object.assign([], { addEventListener() {}, removeEventListener() {}, getTrackById: () => null }); }
    addTextTrack() { return { mode: 'disabled', cues: [], addCue() {}, removeCue() {} }; }
    getVideoPlaybackQuality() {
      const s = state(this);
      return { creationTime: now(), totalVideoFrames: s.frames || 0, droppedVideoFrames: 0, corruptedVideoFrames: 0 };
    }
    canPlayType(type) { return support(type); }

    // ---- methods ----
    load() {
      const s = state(this);
      // Forget the old resource.
      for (const p of s.playPromises.splice(0)) p.reject(new DOMException('The play() request was interrupted by a new load request.', 'AbortError'));
      if (s.loader) { s.loader.stop(); s.loader = null; }
      if (s.source) { s.source._detach(); s.source = null; }
      if (s.open) { send('Vclose ' + this.uid); players.delete(this.uid); s.open = false; }
      const wasLoading = s.networkState === 2 || s.networkState === 1;
      if (wasLoading) later(() => fire(this, 'abort'));
      if (s.networkState !== 0) later(() => fire(this, 'emptied'));
      Object.assign(s, { paused: true, ended: false, seeking: false, ct: 0, lastCt: 0, readyState: 0,
        duration: NaN, vw: 0, vh: 0, error: null, played: [], currentSrc: '', autoplayed: false, ready: 0 });
      // Pick the resource: `src`, else the first `<source>` of a type that plays.
      let url = this.getAttribute('src');
      if (url === null) {
        for (const c of this.children) {
          if (c.localName !== 'source' || c.getAttribute('src') === null) continue;
          const type = c.getAttribute('type');
          if (type && !support(type)) continue;
          url = c.getAttribute('src');
          break;
        }
      }
      if (url === null || String(url).trim() === '') { s.networkState = 0; return; }
      let abs = String(url).trim();
      // A `blob:` address stays as written (it names a MediaSource or a Blob of this page).
      if (!/^(blob|data):/i.test(abs)) {
        try { abs = new URL(abs, location.href).href; } catch (e) { /* kept as written */ }
      }
      s.networkState = 2;
      s.currentSrc = abs;
      later(() => fire(this, 'loadstart'));
      this._open();
      const ms = msUrls.get(abs);
      if (ms) { ms._attach(this); return; }
      if (!/^https?:/i.test(abs) && !/^blob:/i.test(abs) && !/^data:/i.test(abs)) {
        this._fail(4, 'unsupported address');
        return;
      }
      s.loader = /^https?:/i.test(abs) ? new Remote(this, abs) : new Loader(this, abs);
      if (this.preload !== 'none' || this.autoplay) s.loader.start();
    }
    play() {
      const s = state(this);
      return new Promise((resolve, reject) => {
        if (s.error && s.error.code === 4) { reject(new DOMException('The element has no supported sources.', 'NotSupportedError')); return; }
        if (s.networkState === 0) this.load();
        if (s.loader && !s.loader.started) s.loader.start();
        if (!s.paused && s.readyState >= 3) { resolve(); return; }
        s.playPromises.push({ resolve, reject });
        if (s.paused) {
          s.paused = false;
          if (s.ended) { s.ended = false; s.ct = 0; s.lastCt = 0; }
        }
        this._cmd('play');
      });
    }
    pause() {
      const s = state(this);
      if (s.networkState === 0) this.load();
      if (!s.paused) {
        s.paused = true;
        for (const p of s.playPromises.splice(0)) p.reject(new DOMException('The play() request was interrupted by a call to pause().', 'AbortError'));
      }
      this._cmd('pause');
    }

    // ---- with the browser ----
    _cmd(op, ...args) {
      const s = state(this);
      if (s.open) send('V' + op + ' ' + this.uid + (args.length ? ' ' + args.join(' ') : ''));
    }
    _open() {
      const s = state(this);
      if (s.open) return;
      players.set(this.uid, this);
      s.open = true;
      send('Vopen ' + this.uid);
      this._sendVolume();
      if (this.loop) this._cmd('loop', 1);
    }
    _sendVolume() {
      const s = state(this);
      this._cmd('vol', Math.round(s.volume * 100), this.muted ? 1 : 0);
    }
    _seek(t) {
      const s = state(this);
      s.seeking = true;
      s.ended = false;
      s.ct = t;
      s.lastCt = t;
      s.ctAt = now();
      this._cmd('seek', ns(t));
      if (s.loader) s.loader.seek(t);
    }
    _fail(code, msg) {
      const s = state(this);
      if (s.error) return;
      s.error = new MediaError(code, msg);
      s.networkState = code === 4 ? 3 : 1;
      for (const p of s.playPromises.splice(0)) p.reject(new DOMException(msg || 'media error', code === 4 ? 'NotSupportedError' : 'AbortError'));
      later(() => fire(this, 'error'));
    }
    // A state message from the browser: mirror it, then fire its events in order.
    _update(m) {
      const s = state(this);
      const ev = m.ev || [];
      const has = (e) => ev.includes(e);
      if (has('play') || has('pause') || has('ended')) s.paused = !!m.p;
      if (m.ct >= 0) {
        const t = m.ct / NS;
        if (!s.paused && !s.seeking && s.readyState >= 3 && t > s.ct && t - s.ct < 2) s.played = addRange(s.played, s.ct, t);
        if (!s.seeking || has('seeked')) {
          s.ct = t;
          s.ctAt = now();
          // A jump back (a seek, a loop) starts the time over; otherwise it never goes back.
          if (has('seeked') || t < s.lastCt - 1) s.lastCt = t;
        }
      }
      s.ended = !!m.e;
      if (has('seeking')) s.seeking = true;
      if (has('seeked')) s.seeking = false;
      s.readyState = m.rs | 0;
      if (m.w) { s.vw = m.w; s.vh = m.h; }
      if (m.d === -1) s.duration = Infinity; else if (m.d >= 0) s.duration = m.d / NS;
      if (has('volumechange')) { s.volume = (m.vol | 0) / 100; s.muted = !!m.mu; }
      if (m.err && !s.error) {
        s.error = new MediaError(m.err, ['', 'aborted', 'network error', 'decode error', 'format not supported'][m.err] || 'error');
        s.networkState = m.err === 4 ? 3 : 1;
      }
      if (has('loadedmetadata') && s.startAt > 0) { const t = s.startAt; s.startAt = 0; later(() => this._seek(t)); }
      for (const e of ev) {
        if (e === 'loadeddata' || e === 'canplaythrough') s.networkState = s.loader && !s.loader.done ? 2 : 1;
        fire(this, e);
        if (e === 'playing') for (const p of s.playPromises.splice(0)) p.resolve();
        if (e === 'pause' || e === 'ended') for (const p of s.playPromises.splice(0)) p.reject(new DOMException('The play() request was interrupted.', 'AbortError'));
        if (e === 'error') for (const p of s.playPromises.splice(0)) p.reject(new DOMException('decode error', 'NotSupportedError'));
        if (e === 'canplay' && this.autoplay && s.paused && !s.autoplayed) { s.autoplayed = true; this.play().catch(() => {}); }
      }
      if (s.loader) s.loader.tick();
    }
  }
  handlerProps(HTMLMediaElement.prototype, MEDIA_EVENTS);
  for (const [k, v] of Object.entries({ NETWORK_EMPTY: 0, NETWORK_IDLE: 1, NETWORK_LOADING: 2, NETWORK_NO_SOURCE: 3,
    HAVE_NOTHING: 0, HAVE_METADATA: 1, HAVE_CURRENT_DATA: 2, HAVE_FUTURE_DATA: 3, HAVE_ENOUGH_DATA: 4 })) {
    HTMLMediaElement[k] = v;
    Object.defineProperty(HTMLMediaElement.prototype, k, { value: v });
  }
  class HTMLVideoElement extends HTMLMediaElement {
    get width() { return Number(this.getAttribute('width')) || 0; }
    set width(v) { this.setAttribute('width', String(v)); }
    get height() { return Number(this.getAttribute('height')) || 0; }
    set height(v) { this.setAttribute('height', String(v)); }
    get poster() { const v = this.getAttribute('poster'); return v === null ? '' : new URL(v, location.href).href; }
    set poster(v) { this.setAttribute('poster', v); }
  }
  class HTMLAudioElement extends HTMLMediaElement {}
  kjs.elementClasses.set('video', HTMLVideoElement);
  kjs.elementClasses.set('audio', HTMLAudioElement);
  function Audio(src) {
    const a = document.createElement('audio');
    a.preload = 'auto';
    if (src !== undefined) a.src = src;
    return a;
  }
  Audio.prototype = HTMLAudioElement.prototype;

  // ---- appending bytes to a source buffer of the browser's player ----

  // One source buffer's line to the browser: appends go in pieces of at most 1 MiB, each acknowledged
  // (with what the buffer holds) before the next goes, so the decoder's queue stays small.
  const PIECE = 1 << 20;
  let nextSeq = 1;
  class Line {
    constructor(el, src) { this.el = el; this.src = src; this.queue = []; this.waiting = 0; this.ranges = []; this.onDone = null; this.sawInit = false; this.reset = false; }
    // Append `bytes` (Uint8Array); `done(status)` when all of it is in.
    append(bytes, done) {
      const isInit = bytes.length >= 4 && bytes[0] === 0x1a && bytes[1] === 0x45 && bytes[2] === 0xdf && bytes[3] === 0xa3;
      if (isInit && this.sawInit) send('Vreset ' + this.el.uid + ' ' + this.src + ' -1');
      else if (this.reset && !isInit) send('Vreset ' + this.el.uid + ' ' + this.src + ' 0');
      this.reset = false;
      if (isInit) this.sawInit = true;
      for (let i = 0; i < bytes.length; i += PIECE) this.queue.push(bytes.subarray(i, Math.min(bytes.length, i + PIECE)));
      if (!bytes.length) this.queue.push(bytes);
      this.onDone = done;
      this.status = 0;
      this.next();
    }
    next() {
      if (this.waiting) return;
      const piece = this.queue.shift();
      if (!piece) { const d = this.onDone; this.onDone = null; if (d) d(this.status); return; }
      const seq = nextSeq++;
      this.waiting = seq;
      const copy = piece.slice();
      const r = sendBytes('v' + this.el.uid + ' ' + this.src + ' ' + seq + '\n', copy.buffer);
      if (r < 0) { this.waiting = 0; this.queue.length = 0; this.status = 2; this.next(); }
    }
    // The browser's answer to an append (or a remove: `seq` 0).
    answered(seq, status, ranges) {
      this.ranges = fromNs(ranges);
      if (seq === 0) { const d = this.onRemoved; this.onRemoved = null; if (d) d(); return; }
      if (seq !== this.waiting) return;
      this.waiting = 0;
      if (status) { this.status = status; this.queue.length = 0; }
      this.next();
    }
    cancel() { this.queue.length = 0; this.reset = true; }
    remove(a, b, done) { this.onRemoved = done; send('Vrm ' + this.el.uid + ' ' + this.src + ' ' + ns(a) + ' ' + (b === Infinity ? '9223372036854775807' : ns(b))); }
  }

  // ---- a `src` address: the browser fetches it into the decoder ----

  class Remote {
    constructor(el, url) { this.el = el; this.url = url; this.ranges = []; this.started = false; this.done = false; this.lastProgress = -1e9; this.line = this; }
    start() {
      if (this.started) return;
      this.started = true;
      send('Vsrc ' + this.el.uid + ' ' + this.url);
    }
    stop() {}
    tick() {}
    seek() {}
    // What source 0 holds, after each piece the decoder took.
    answered(seq, status, ranges) {
      this.ranges = fromNs(ranges);
      if (now() - this.lastProgress > 350) { this.lastProgress = now(); fire(this.el, 'progress'); }
    }
  }

  // ---- a `blob:` (of a Blob) or `data:` address: read here, appended to source 0 ----

  const AHEAD = 30;
  class Loader {
    constructor(el, url) { this.el = el; this.url = url; this.line = new Line(el, 0); this.offset = 0; this.total = -1; this.done = false; this.busy = false; this.started = false; this.stopped = false; this.ranged = true; this.ctl = null; this.lastProgress = 0; }
    get ranges() { return this.line.ranges; }
    start() { this.started = true; this.pump(); }
    stop() { this.stopped = true; if (this.ctl) { try { this.ctl.abort(); } catch (e) { /* gone */ } } }
    tick() { if (this.started && !this.busy && !this.done) this.pump(); }
    seek(t) { /* the bytes come in order; a later time is served when they arrive */ }
    ahead() {
      const t = this.el.currentTime;
      const r = this.line.ranges.find(([a, b]) => a <= t + 0.5 && b > t);
      return r ? r[1] - t : 0;
    }
    pump() {
      if (this.stopped || this.busy || this.done) return;
      // Enough ahead: wait for the time to move (`timeupdate` calls tick()).
      if (this.ahead() > AHEAD) return;
      this.busy = true;
      const s = state(this.el);
      s.networkState = 2;
      const ctl = (this.ctl = new AbortController());
      const end = this.offset + PIECE - 1;
      const headers = this.ranged ? { Range: 'bytes=' + this.offset + '-' + end } : {};
      fetch(this.url, { headers, signal: ctl.signal, credentials: 'same-origin' }).then(async (res) => {
        if (this.stopped) return;
        if (!res.ok && res.status !== 206) throw new Error('HTTP ' + res.status);
        if (res.status === 206) {
          const cr = /\/(\d+)\s*$/.exec(res.headers.get('content-range') || '');
          if (cr) this.total = Number(cr[1]);
        } else {
          // No ranges: the whole file in one answer, appended as it streams in.
          this.ranged = false;
          const len = Number(res.headers.get('content-length'));
          if (len > 0) this.total = len;
          if (this.offset > 0) { send('Vreset ' + this.el.uid + ' 0 -1'); this.offset = 0; }
        }
        const reader = res.body.getReader();
        for (;;) {
          const { value, done } = await reader.read();
          if (done || this.stopped) break;
          await new Promise((ok) => this.line.append(value, (st) => { if (st) this.broken = st; ok(); }));
          if (this.broken) throw new Error(this.broken === 1 ? 'the media buffer is full' : 'not a WebM file');
          this.offset += value.length;
          if (now() - this.lastProgress > 350) { this.lastProgress = now(); fire(this.el, 'progress'); }
        }
        if (!this.ranged || (this.total >= 0 && this.offset >= this.total) || res.status !== 206) this.finish();
      }).catch((e) => {
        if (this.stopped) return;
        console.warn('media: ' + this.url + ': ' + (e && e.message));
        this.el._fail(this.broken === 2 ? 4 : 2, String(e && e.message));
      }).finally(() => {
        this.busy = false;
        this.ctl = null;
        if (!this.done && !this.stopped && !state(this.el).error) later(() => this.pump());
      });
    }
    finish() {
      this.done = true;
      const s = state(this.el);
      s.networkState = 1;
      send('Veos ' + this.el.uid + ' 1');
      fire(this.el, 'progress');
      later(() => fire(this.el, 'suspend'));
    }
  }

  // ---- Media Source Extensions ----

  class SourceBufferList extends ETarget {
    constructor() { super(); this._on = null; this._list = []; }
    get length() { return this._list.length; }
    item(i) { return this._list[i] ?? null; }
    [Symbol.iterator]() { return this._list[Symbol.iterator](); }
  }
  handlerProps(SourceBufferList.prototype, ['addsourcebuffer', 'removesourcebuffer']);
  const listProxy = (l) => new Proxy(l, {
    get(t, p, r) { if (typeof p === 'string' && /^\d+$/.test(p)) return t._list[Number(p)]; return Reflect.get(t, p, r); },
  });

  class MediaSource extends ETarget {
    constructor() {
      super();
      this._on = null;
      this.readyState = 'closed';
      this._duration = NaN;
      this._sb = new SourceBufferList();
      this.sourceBuffers = listProxy(this._sb);
      this.activeSourceBuffers = this.sourceBuffers;
      this._el = null;
      this._next = 0;
      this._live = null;
    }
    static isTypeSupported(type) { return support(type) !== ''; }
    static get canConstructInDedicatedWorker() { return false; }
    get handle() { return this; }
    get duration() { return this.readyState === 'closed' ? NaN : this._duration; }
    set duration(v) {
      const d = Number(v);
      if (!(d >= 0)) throw new TypeError('duration must be a non-negative number');
      if (this.readyState !== 'open') throw new DOMException('The MediaSource is not open.', 'InvalidStateError');
      if (this._sb._list.some((b) => b.updating)) throw new DOMException('A SourceBuffer is updating.', 'InvalidStateError');
      this._setDuration(d);
    }
    _setDuration(d) {
      if (d === this._duration) return;
      this._duration = d;
      if (this._el) {
        send('Vdur ' + this._el.uid + ' ' + (isFinite(d) ? ns(d) : -1));
        later(() => fire(this._el, 'durationchange'));
      }
    }
    addSourceBuffer(type) {
      if (!type) throw new TypeError('addSourceBuffer needs a type');
      if (!MediaSource.isTypeSupported(type)) throw new DOMException("The type '" + type + "' is not supported.", 'NotSupportedError');
      if (this.readyState !== 'open') throw new DOMException('The MediaSource is not open.', 'InvalidStateError');
      if (this._next >= 4) throw new DOMException('Too many SourceBuffers.', 'QuotaExceededError');
      const sb = new SourceBuffer(this, String(type), this._next++);
      this._sb._list.push(sb);
      later(() => fire(this._sb, 'addsourcebuffer'));
      return sb;
    }
    removeSourceBuffer(sb) {
      const i = this._sb._list.indexOf(sb);
      if (i < 0) throw new DOMException('Not a SourceBuffer of this MediaSource.', 'NotFoundError');
      if (sb.updating) sb.abort();
      sb._line.remove(0, Infinity, () => {});
      sb._removed = true;
      this._sb._list.splice(i, 1);
      later(() => fire(this._sb, 'removesourcebuffer'));
    }
    endOfStream(error) {
      if (this.readyState !== 'open') throw new DOMException('The MediaSource is not open.', 'InvalidStateError');
      if (this._sb._list.some((b) => b.updating)) throw new DOMException('A SourceBuffer is updating.', 'InvalidStateError');
      this.readyState = 'ended';
      later(() => fire(this, 'sourceended'));
      if (!this._el) return;
      if (error === 'network') { this._el._fail(2, 'network error'); return; }
      if (error === 'decode') { this._el._fail(3, 'decode error'); return; }
      // The duration becomes the end of what is buffered.
      const end = this._buffered().reduce((m, r) => Math.max(m, r[1]), 0);
      if (end > 0 && !(end === this._duration)) this._setDuration(end);
      send('Veos ' + this._el.uid + ' 1');
    }
    setLiveSeekableRange(a, b) { this._live = [Number(a), Number(b)]; }
    clearLiveSeekableRange() { this._live = null; }
    // The element's buffered: the time every source buffer holds (the last range of each runs to
    // the end once the stream ended).
    _buffered() {
      const lists = this._sb._list.map((b) => b._line.ranges.slice());
      if (this.readyState === 'ended') {
        const end = Math.max(0, ...lists.map((l) => (l.length ? l[l.length - 1][1] : 0)));
        for (const l of lists) if (l.length) l[l.length - 1] = [l[l.length - 1][0], end];
      }
      return intersect(lists);
    }
    _attach(el) {
      if (this._el) { el._fail(4, 'the MediaSource is already in use'); return; }
      this._el = el;
      state(el).source = this;
      this.readyState = 'open';
      later(() => fire(this, 'sourceopen'));
    }
    _detach() {
      for (const b of this._sb._list) { b._removed = true; b._line.cancel(); }
      this._sb._list.length = 0;
      this._el = null;
      if (this.readyState !== 'closed') {
        this.readyState = 'closed';
        later(() => fire(this, 'sourceclose'));
      }
    }
    _reopen() {
      if (this.readyState === 'ended') {
        this.readyState = 'open';
        if (this._el) send('Veos ' + this._el.uid + ' 0');
        later(() => fire(this, 'sourceopen'));
      }
    }
  }
  handlerProps(MediaSource.prototype, ['sourceopen', 'sourceended', 'sourceclose']);

  class SourceBuffer extends ETarget {
    constructor(ms, type, index) {
      super();
      this._on = null;
      this._ms = ms;
      this._type = type;
      this._index = index;
      this._line = new Line(ms._el, index);
      this._removed = false;
      this.updating = false;
      this._mode = 'segments';
      this._tso = 0;
      this.appendWindowStart = 0;
      this.appendWindowEnd = Infinity;
      this.audioTracks = [];
      this.videoTracks = [];
      this.textTracks = [];
    }
    get mode() { return this._mode; }
    set mode(v) {
      if (v !== 'segments' && v !== 'sequence') throw new TypeError('bad mode');
      this._check();
      this._mode = v;
    }
    get timestampOffset() { return this._tso; }
    set timestampOffset(v) {
      this._check();
      const t = Number(v);
      this._tso = t;
      send('Vtso ' + this._ms._el.uid + ' ' + this._index + ' ' + ns(t));
    }
    get buffered() {
      if (this._removed) throw new DOMException('This SourceBuffer has been removed.', 'InvalidStateError');
      return new TimeRanges(this._line.ranges.slice());
    }
    _check() {
      if (this._removed || !this._ms._el) throw new DOMException('This SourceBuffer has been removed.', 'InvalidStateError');
      if (this.updating) throw new DOMException('This SourceBuffer is still processing an append or remove.', 'InvalidStateError');
    }
    appendBuffer(data) {
      this._check();
      let bytes;
      if (data instanceof ArrayBuffer) bytes = new Uint8Array(data);
      else if (ArrayBuffer.isView(data)) bytes = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
      else throw new TypeError('appendBuffer needs an ArrayBuffer or a view');
      this._ms._reopen();
      this.updating = true;
      later(() => fire(this, 'updatestart'));
      // The bytes are copied now: the page may reuse its buffer.
      bytes = bytes.slice();
      this._line.append(bytes, (status) => {
        if (!this.updating) return;
        this.updating = false;
        if (status) {
          // The append error algorithm: `error`, `updateend`, and a decode error on the element.
          fire(this, 'error');
          fire(this, 'updateend');
          if (status === 2 && this._ms._el) {
            try { this._ms.endOfStream('decode'); } catch (e) { /* not open */ }
          }
          return;
        }
        fire(this, 'update');
        fire(this, 'updateend');
        const el = this._ms._el;
        if (el) fire(el, 'progress');
      });
    }
    appendBufferAsync(data) {
      return new Promise((ok, fail) => {
        const done = () => { this.removeEventListener('updateend', done); ok(); };
        this.addEventListener('updateend', done);
        try { this.appendBuffer(data); } catch (e) { this.removeEventListener('updateend', done); fail(e); }
      });
    }
    abort() {
      if (this._removed) throw new DOMException('This SourceBuffer has been removed.', 'InvalidStateError');
      this._line.cancel();
      if (this.updating) {
        this.updating = false;
        fire(this, 'abort');
        fire(this, 'updateend');
      }
      this.appendWindowStart = 0;
      this.appendWindowEnd = Infinity;
    }
    remove(start, end) {
      this._check();
      const a = Number(start), b = Number(end);
      if (!(a >= 0) || !(b > a)) throw new TypeError('bad remove range');
      this._ms._reopen();
      this.updating = true;
      later(() => fire(this, 'updatestart'));
      this._line.remove(a, b, () => {
        if (!this.updating) return;
        this.updating = false;
        fire(this, 'update');
        fire(this, 'updateend');
      });
    }
    changeType(type) {
      if (!MediaSource.isTypeSupported(type)) throw new DOMException("The type '" + type + "' is not supported.", 'NotSupportedError');
      this._check();
      this._type = String(type);
    }
  }
  handlerProps(SourceBuffer.prototype, ['updatestart', 'update', 'updateend', 'error', 'abort']);

  // `URL.createObjectURL(mediaSource)` gives the `blob:` address a media element takes as `src`.
  const createObjectURL = URL.createObjectURL;
  const revokeObjectURL = URL.revokeObjectURL;
  URL.createObjectURL = function (obj) {
    if (obj instanceof MediaSource) {
      const u = 'blob:' + location.origin + '/' + crypto.randomUUID();
      msUrls.set(u, obj);
      return u;
    }
    return createObjectURL(obj);
  };
  URL.revokeObjectURL = function (u) { msUrls.delete(String(u)); revokeObjectURL(u); };

  // ---- what the browser says ----

  function onMedia(m) {
    const el = players.get(m.id);
    if (!el) return;
    const s = state(el);
    if (m.buf) {
      const [src, seq, status, ranges] = m.buf;
      const line = s.loader && src === 0 ? s.loader.line : s.source && s.source._sb._list.find((b) => b._index === src)?._line;
      if (line) line.answered(seq, status, ranges);
      return;
    }
    if (m.fail) { el._fail(m.fail[0], m.fail[1]); return; }
    if (m.cue) return;
    el._update(m);
  }

  const baseOnMessage = kjs.onMessage;
  kjs.onMessage = function (text) {
    if (text.startsWith('{"t":"media"')) {
      try { onMedia(JSON.parse(text)); } catch (e) { report(e); }
      return;
    }
    baseOnMessage(text);
  };

  // The media elements of the document load once it is parsed (a script may have done it before).
  document.addEventListener('DOMContentLoaded', () => {
    for (const el of document.querySelectorAll('video, audio')) {
      const s = state(el);
      if (s.networkState === 0) el.load();
    }
  });

  for (const [k, v] of Object.entries({
    HTMLMediaElement, HTMLVideoElement, HTMLAudioElement, Audio, MediaSource, SourceBuffer, SourceBufferList,
    TimeRanges, MediaError,
  })) define(k, v);
})(globalThis, globalThis.__kjs_native, globalThis.__kjs);
