// Kitsune jsrun --page: the rest of the window that big real-world players and apps reach for
// (brought up with YouTube's embedded player, docs/design/tls-browser.md §16), evaluated last.
//
// Each API either works or says honestly that it is not there: `Image` and `Option` make elements,
// `canvas.getContext` gives null (no canvas yet, which is what a browser says for a context it does
// not support), `navigator.mediaCapabilities` answers from what the decoder plays (VP9 profile 0 and
// Opus in WebM), `navigator.mediaSession` keeps what the page sets.
(function (global, native, kjs) {
  'use strict';
  const D = kjs.dom;
  const { document } = D;
  const define = (name, value) => {
    Object.defineProperty(global, name, { value, writable: true, configurable: true, enumerable: false });
  };

  // ---- element constructors ----

  function Image(width, height) {
    const el = document.createElement('img');
    if (width !== undefined) el.setAttribute('width', String(width >>> 0));
    if (height !== undefined) el.setAttribute('height', String(height >>> 0));
    return el;
  }
  Image.prototype = global.HTMLImageElement.prototype;
  define('Image', Image);
  function Option(text, value, defaultSelected, selected) {
    const el = document.createElement('option');
    if (text !== undefined) el.textContent = String(text);
    if (value !== undefined) el.setAttribute('value', String(value));
    if (defaultSelected || selected) el.setAttribute('selected', '');
    return el;
  }
  Option.prototype = global.HTMLOptionElement.prototype;
  define('Option', Option);

  // ---- canvas: not drawn yet ----

  const CP = global.HTMLCanvasElement.prototype;
  for (const [k, attr, dflt] of [['width', 'width', 300], ['height', 'height', 150]]) {
    Object.defineProperty(CP, k, { configurable: true,
      get() { const v = parseInt(this.getAttribute(attr), 10); return v >= 0 ? v : dflt; },
      set(v) { this.setAttribute(attr, String(v >>> 0)); } });
  }
  CP.getContext = function () { return null; };
  CP.toDataURL = function () { return 'data:,'; };
  CP.toBlob = function (cb) { setTimeout(() => cb(null), 0); };
  CP.captureStream = undefined;

  // ---- media capabilities ----

  const media = (type) => {
    const t = String(type || '').toLowerCase();
    return /^(video|audio)\/webm\b/.test(t) && global.MediaSource && global.MediaSource.isTypeSupported(t);
  };
  const mediaCapabilities = {
    decodingInfo(cfg) {
      cfg = cfg || {};
      const v = cfg.video, a = cfg.audio;
      if (!v && !a) return Promise.reject(new TypeError('decodingInfo: no audio or video configuration'));
      let ok = (!v || media(v.contentType)) && (!a || media(a.contentType));
      // The decoder is software: say a picture over 1080p, or over 30 fps above 480p, is not smooth.
      const big = v && ((v.width | 0) * (v.height | 0) > 1920 * 1080);
      const smooth = ok && !(v && (big || ((v.framerate || 30) > 30 && (v.height | 0) > 480)));
      if (cfg.keySystemConfiguration) ok = false; // no EME
      return Promise.resolve({ supported: ok, smooth, powerEfficient: false, keySystemAccess: null, configuration: cfg });
    },
    encodingInfo() { return Promise.resolve({ supported: false, smooth: false, powerEfficient: false }); },
  };
  const nav = global.navigator;
  Object.defineProperty(nav, 'mediaCapabilities', { configurable: true, value: mediaCapabilities });
  define('MediaCapabilities', function MediaCapabilities() { throw new TypeError('Illegal constructor'); });

  // ---- media session: kept, shown nowhere ----

  class MediaMetadata {
    constructor(init) {
      init = init || {};
      this.title = String(init.title || '');
      this.artist = String(init.artist || '');
      this.album = String(init.album || '');
      this.artwork = Array.isArray(init.artwork) ? init.artwork.slice() : [];
    }
  }
  const handlers = new Map();
  const mediaSession = {
    metadata: null, playbackState: 'none',
    setActionHandler(action, fn) { if (fn) handlers.set(String(action), fn); else handlers.delete(String(action)); },
    setPositionState() {}, setMicrophoneActive() {}, setCameraActive() {},
  };
  if (!('mediaSession' in nav)) Object.defineProperty(nav, 'mediaSession', { configurable: true, value: mediaSession });
  define('MediaMetadata', MediaMetadata);
})(globalThis, globalThis.__kjs_native, globalThis.__kjs);
