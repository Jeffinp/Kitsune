// Host check of <video>/<audio> and Media Source Extensions in JavaScript
// (user-apps/jsrun/src/bin/jsrun/{dom,net,media,compat}.js) with Node:
//   node tools/js-media-check.js user-apps/jsrun/src/bin/jsrun
// A fake browser answers the M/m messages (the player's state, what a source buffer holds) and the
// fetches of a `src`; prints ok/FAIL per check and exits 1 on a failure.
const vm = require('vm');
const fs = require('fs');
const nodeCrypto = require('crypto');
const dir = process.argv[2];
const src = ['dom.js', 'net.js', 'media.js', 'compat.js'].map((f) => fs.readFileSync(dir + '/' + f, 'utf8')).join('\n');
const sent = [];
const t0 = Date.now();
const ctx = { console, performance: { now: () => Date.now() - t0 } };
ctx.globalThis = ctx;
vm.createContext(ctx);
ctx.__kjs_native = {
  send: (s) => { sent.push(Buffer.from(s, 'utf8')); return 0; },
  sendBytes: (h, ab) => { sent.push(Buffer.concat([Buffer.from(h, 'utf8'), ab ? Buffer.from(ab) : Buffer.alloc(0)])); return 0; },
  random: (n) => { const b = nodeCrypto.randomBytes(n); return b.buffer.slice(b.byteOffset, b.byteOffset + n); },
  evalScript: (code, name) => { vm.runInContext(code, ctx, { filename: name }); return true; },
};
vm.runInContext(src, ctx, { filename: 'prelude' });
const kjs = ctx.__kjs;
// Each piece of page code in its own block: its `const`s do not leak into the next.
const run = (code) => vm.runInContext('{' + code + '\n}', ctx);
const tick = async () => {
  for (let i = 0; i < 40; i++) {
    kjs.runOne();
    await new Promise((r) => setImmediate(r));
  }
};
const settle = async () => { for (let i = 0; i < 5; i++) { await new Promise((r) => setTimeout(r, 2)); await tick(); } };
const toAB = (b) => b.buffer.slice(b.byteOffset, b.byteOffset + b.length);
let failures = 0;
const check = (ok, what) => { console.log((ok ? 'ok   ' : 'FAIL ') + what); if (!ok) failures++; };

// What the page sent: M commands as text, m data as {id, src, seq, n}, F requests parsed.
function take() {
  const out = [];
  for (const m of sent.splice(0)) {
    if (m[0] === 0x56) out.push({ cmd: m.toString().slice(1) });
    else if (m[0] === 0x76) {
      const nl = m.indexOf(10);
      const [id, s, seq] = m.slice(1, nl).toString().split(' ').map(Number);
      out.push({ data: true, id, src: s, seq, n: m.length - nl - 1, bytes: m.slice(nl + 1) });
    } else if (m[0] === 0x46) {
      const lines = m.toString('latin1').slice(1).split('\n');
      const headers = [];
      for (let i = 7; lines[i] !== ''; i++) headers.push(lines[i]);
      out.push({ fetch: true, id: +lines[0], url: lines[2], headers });
    } else out.push({ other: m.toString() });
  }
  return out;
}
const media = (o) => kjs.onMessage(JSON.stringify({ t: 'media', ...o }));
const state = (id, ev, extra = {}) => media({ id, ev, ct: 0, p: 1, e: 0, sk: 0, rs: 0, w: 0, h: 0, v: 1, a: 1, d: -2, vol: 100, mu: 0, err: 0, ...extra });
const S = 1e9;

(async () => {
  kjs.onMessage(JSON.stringify({ t: 'init', url: 'http://h:8079/v', ua: 'UA', lang: 'pt-BR', cookie: '', w: 800, h: 600, quirks: false,
    dom: [['html', 1, {}, [['head', 2, {}, []], ['body', 3, {}, [['video', 4, { src: '/media/a.webm', controls: '' }, []],
      ['video', 5, {}, [['source', 6, { src: '/x.mp4', type: 'video/mp4' }, []], ['source', 7, { src: '/y.webm', type: 'video/webm' }, []]]]]]]]],
    scripts: [] }));
  await settle();

  // ---- types ----
  check(run(`document.createElement('video') instanceof HTMLVideoElement && document.createElement('audio') instanceof HTMLMediaElement`), 'video and audio elements have their classes');
  check(run(`document.createElement('video').canPlayType('video/webm; codecs="vp9, opus"')`) === 'probably', 'canPlayType webm vp9+opus');
  check(run(`document.createElement('video').canPlayType('video/webm')`) === 'maybe', 'canPlayType webm alone');
  check(run(`document.createElement('video').canPlayType('video/mp4; codecs="avc1.42E01E"')`) === '', 'canPlayType mp4 is not');
  check(run(`document.createElement('video').canPlayType('video/webm; codecs="vp09.02.10.10"')`) === '', 'VP9 profile 2 is not');
  check(run(`MediaSource.isTypeSupported('audio/webm; codecs="opus"') && MediaSource.isTypeSupported('video/webm; codecs="vp09.00.51.08"') && !MediaSource.isTypeSupported('video/mp4')`), 'MediaSource.isTypeSupported');

  // ---- a src in the document loads on DOMContentLoaded; the <source> that plays is picked ----
  let out = take();
  const opens = out.filter((o) => o.cmd && o.cmd.startsWith('open')).map((o) => o.cmd);
  check(opens.includes('open 4') && opens.includes('open 5'), 'both players opened: ' + JSON.stringify(opens));
  const srcs = out.filter((o) => o.cmd && o.cmd.startsWith('src')).map((o) => o.cmd);
  check(srcs.includes('src 4 http://h:8079/media/a.webm') && srcs.includes('src 5 http://h:8079/y.webm'),
    'the browser fetches the sources itself: ' + JSON.stringify(srcs));
  check(!out.some((o) => o.fetch || o.data), 'the page fetches nothing and sends no bytes');
  check(run(`document.querySelectorAll('video')[1].currentSrc`) === 'http://h:8079/y.webm', 'currentSrc of the <source>');
  run(`globalThis.log = []; const v = document.querySelector('video'); for (const e of ['loadstart','progress','loadedmetadata','loadeddata','canplay','canplaythrough','play','playing','pause','seeking','seeked','timeupdate','ended','volumechange','durationchange','waiting','suspend','error']) v.addEventListener(e, () => log.push(e)); v.onended = () => log.push('onended');`);
  // What the decoder holds, as the download goes.
  media({ id: 4, buf: [0, 0, 0, [[0, 3 * S]]] });
  await settle();
  check(run(`document.querySelector('video').buffered.length === 1 && document.querySelector('video').buffered.end(0) === 3`), 'buffered from the decoder');
  check(run('log').includes('progress'), 'progress');
  // The browser: metadata, data, enough.
  state(4, ['durationchange', 'loadedmetadata'], { rs: 1, w: 640, h: 360, d: 3 * S });
  state(4, ['loadeddata', 'canplay', 'canplaythrough'], { rs: 4, w: 640, h: 360, d: 3 * S });
  await settle();
  check(run(`const v = document.querySelector('video'); v.duration === 3 && v.videoWidth === 640 && v.readyState === 4 && v.paused`), 'duration, size, readyState');
  // play(): the promise resolves on playing.
  run(`globalThis.played = 'no'; document.querySelector('video').play().then(() => played = 'yes', (e) => played = 'rejected ' + e.name);`);
  out = take();
  check(out.some((o) => o.cmd === 'play 4'), 'play goes to the browser');
  check(run(`document.querySelector('video').paused`) === false, 'paused is false at once');
  state(4, ['play', 'playing'], { rs: 4, p: 0, w: 640, h: 360, d: 3 * S });
  await settle();
  check(ctx.played === 'yes', 'play() resolved');
  state(4, ['timeupdate'], { rs: 4, p: 0, ct: 1.5 * S, w: 640, h: 360, d: 3 * S });
  await settle();
  const ct = run(`document.querySelector('video').currentTime`);
  check(ct >= 1.5 && ct < 2.1, 'currentTime follows: ' + ct);
  // A seek.
  run(`document.querySelector('video').currentTime = 0.5`);
  out = take();
  check(out.some((o) => o.cmd === 'seek 4 500000000'), 'seek goes in ns');
  check(run(`document.querySelector('video').seeking && document.querySelector('video').currentTime === 0.5`), 'seeking at the new time');
  state(4, ['seeking'], { rs: 1, p: 0, ct: 0.5 * S, sk: 1, w: 640, h: 360, d: 3 * S });
  state(4, ['timeupdate', 'seeked', 'canplay', 'playing'], { rs: 4, p: 0, ct: 0.5 * S, w: 640, h: 360, d: 3 * S });
  await settle();
  check(!run(`document.querySelector('video').seeking`), 'seeked');
  // Volume.
  run(`const v = document.querySelector('video'); v.volume = 0.25; v.muted = true;`);
  out = take();
  check(out.some((o) => o.cmd === 'vol 4 25 0') && out.some((o) => o.cmd === 'vol 4 25 1'), 'volume and mute go to the browser');
  let threw = false;
  try { run(`document.querySelector('video').volume = 2`); } catch (e) { threw = e.name === 'IndexSizeError'; }
  check(threw, 'volume out of range throws IndexSizeError');
  // The end.
  state(4, ['timeupdate', 'pause', 'ended'], { rs: 4, p: 1, e: 1, ct: 3 * S, w: 640, h: 360, d: 3 * S });
  await settle();
  check(run(`const v = document.querySelector('video'); v.ended && v.paused && v.currentTime === 3`), 'ended');
  const log = run('log');
  const order = ['loadedmetadata', 'loadeddata', 'canplay', 'canplaythrough', 'play', 'playing', 'seeking', 'seeked', 'volumechange', 'pause', 'ended', 'onended'];
  check(order.every((e) => log.includes(e)) && log.indexOf('loadedmetadata') < log.indexOf('loadeddata') && log.indexOf('pause') < log.indexOf('ended'), 'events: ' + log.join(','));
  check(run(`document.querySelector('video').played.length >= 1`), 'played ranges');
  // pause() rejects a pending play().
  run(`globalThis.p2 = 'no'; const v = document.querySelectorAll('video')[1]; v.play().then(() => p2 = 'yes', (e) => p2 = e.name); v.pause();`);
  await settle();
  check(ctx.p2 === 'AbortError', 'pause() rejects a pending play(): ' + ctx.p2);
  take();

  // ---- Media Source Extensions ----
  run(`
    globalThis.ms = new MediaSource();
    globalThis.msLog = [];
    globalThis.mv = document.createElement('video');
    document.body.appendChild(mv);
    ms.addEventListener('sourceopen', () => {
      msLog.push('sourceopen:' + ms.readyState);
      globalThis.vb = ms.addSourceBuffer('video/webm; codecs="vp9"');
      globalThis.ab = ms.addSourceBuffer('audio/webm; codecs="opus"');
      vb.addEventListener('updateend', () => msLog.push('v-updateend'));
      ab.addEventListener('updateend', () => msLog.push('a-updateend'));
      vb.addEventListener('updatestart', () => msLog.push('v-updatestart'));
      ms.duration = 10;
    });
    ms.addEventListener('sourceended', () => msLog.push('sourceended'));
    ms.addEventListener('sourceclose', () => msLog.push('sourceclose'));
    globalThis.url = URL.createObjectURL(ms);
    mv.src = url;
  `);
  await settle();
  out = take();
  const mid = run('mv.uid');
  check(/^blob:http:\/\/h:8079\//.test(ctx.url), 'createObjectURL(MediaSource) is a blob: address');
  check(out.some((o) => o.cmd === 'open ' + mid) && !out.some((o) => o.fetch), 'the player opens and nothing is fetched');
  check(ctx.msLog[0] === 'sourceopen:open' && run('ms.sourceBuffers.length') === 2 && run('ms.sourceBuffers[0] === vb'), 'sourceopen, two source buffers');
  check(out.some((o) => o.cmd === 'dur ' + mid + ' 10000000000'), 'the duration goes to the browser');
  let bad = '';
  try { run(`ms.addSourceBuffer('video/mp4; codecs="avc1"')`); } catch (e) { bad = e.name; }
  check(bad === 'NotSupportedError', 'addSourceBuffer of mp4: ' + bad);
  // Two appends: the video init + a cluster in one, the audio in another, in pieces of 1 MiB.
  const big = Buffer.alloc(2.5 * 1024 * 1024, 3);
  big[0] = 0x1a; big[1] = 0x45; big[2] = 0xdf; big[3] = 0xa3;
  ctx.bigVideo = new Uint8Array(big);
  run(`vb.appendBuffer(bigVideo); ab.appendBuffer(new Uint8Array([0x1a, 0x45, 0xdf, 0xa3, 9, 9]));`);
  check(run('vb.updating && ab.updating'), 'updating while the append is on its way');
  threw = false;
  try { run('vb.appendBuffer(new Uint8Array(4))'); } catch (e) { threw = e.name === 'InvalidStateError'; }
  check(threw, 'a second append while updating throws InvalidStateError');
  await settle();
  out = take();
  let vd = out.filter((o) => o.data && o.src === 0);
  const ad = out.filter((o) => o.data && o.src === 1);
  check(vd.length === 1 && vd[0].n === 1024 * 1024 && ad.length === 1 && ad[0].n === 6, 'one piece of each in flight first');
  media({ id: mid, buf: [0, vd[0].seq, 0, [[0, 2 * S]]] });
  await settle();
  vd = take().filter((o) => o.data);
  check(vd.length === 1 && vd[0].n === 1024 * 1024, 'the next piece after the answer');
  media({ id: mid, buf: [0, vd[0].seq, 0, [[0, 4 * S]]] });
  await settle();
  vd = take().filter((o) => o.data);
  check(vd.length === 1 && vd[0].n === 512 * 1024, 'the last piece');
  media({ id: mid, buf: [0, vd[0].seq, 0, [[0, 6 * S]]] });
  media({ id: mid, buf: [1, ad[0].seq, 0, [[0, 5 * S]]] });
  await settle();
  check(!run('vb.updating') && !run('ab.updating') && ctx.msLog.includes('v-updateend') && ctx.msLog.includes('a-updateend'), 'updateend for both');
  check(run('vb.buffered.end(0)') === 6 && run('ab.buffered.end(0)') === 5, 'each buffer holds its own');
  check(run('mv.buffered.length === 1 && mv.buffered.end(0) === 5'), 'the element holds what both hold');
  // A new init segment on a buffer that had one: the parser starts over.
  run(`vb.appendBuffer(new Uint8Array([0x1a, 0x45, 0xdf, 0xa3, 1]))`);
  await settle();
  out = take();
  check(out.some((o) => o.cmd === 'reset ' + mid + ' 0 -1'), 'a second init segment resets the parser');
  media({ id: mid, buf: [0, out.find((o) => o.data).seq, 0, [[0, 6 * S]]] });
  await settle();
  // remove().
  run(`vb.remove(0, 2)`);
  out = take();
  check(out.some((o) => o.cmd === 'rm ' + mid + ' 0 0 2000000000') && run('vb.updating'), 'remove goes to the browser');
  media({ id: mid, buf: [0, 0, 0, [[2 * S, 6 * S]]] });
  await settle();
  check(!run('vb.updating') && run('vb.buffered.start(0)') === 2, 'remove answered');
  // A broken append: error, updateend, and the element gets a decode error.
  run(`globalThis.sbErr = 0; ab.addEventListener('error', () => sbErr++); ab.appendBuffer(new Uint8Array([1, 2, 3]));`);
  await settle();
  const bd = take().find((o) => o.data);
  // endOfStream first, with the good stream (the broken one comes after).
  run(`globalThis.eosOk = true;`);
  media({ id: mid, buf: [1, bd.seq, 2, [[0, 5 * S]]] });
  await settle();
  check(ctx.sbErr === 1 && run('mv.error && mv.error.code === 3'), 'a broken append is a decode error');
  // A fresh MediaSource: endOfStream sets the duration to what is buffered and says eos.
  run(`
    globalThis.ms2 = new MediaSource(); globalThis.mv2 = document.createElement('video');
    ms2.onsourceopen = () => { globalThis.sb2 = ms2.addSourceBuffer('video/webm; codecs="vp9"'); };
    mv2.src = URL.createObjectURL(ms2);`);
  await settle();
  const m2 = run('mv2.uid');
  run(`sb2.appendBuffer(new Uint8Array([0x1a, 0x45, 0xdf, 0xa3]))`);
  await settle();
  media({ id: m2, buf: [0, take().find((o) => o.data).seq, 0, [[0, 2.5 * S]]] });
  await settle();
  run(`ms2.endOfStream()`);
  await settle();
  out = take();
  check(run(`ms2.readyState`) === 'ended' && out.some((o) => o.cmd === 'eos ' + m2 + ' 1') && run('ms2.duration') === 2.5, 'endOfStream: ended, eos, duration ' + run('ms2.duration'));
  // Appending after the end reopens.
  run(`sb2.appendBuffer(new Uint8Array([0]))`);
  await settle();
  out = take();
  check(run(`ms2.readyState`) === 'open' && out.some((o) => o.cmd === 'eos ' + m2 + ' 0'), 'an append reopens the source');
  // A new src closes the MediaSource.
  run(`globalThis.closed2 = 0; ms2.onsourceclose = () => closed2++; mv2.src = '';`);
  await settle();
  out = take();
  check(ctx.closed2 === 1 && run('ms2.readyState') === 'closed' && out.some((o) => o.cmd === 'close ' + m2), 'sourceclose and the player closes');

  // ---- a Blob source: read here, appended to source 0, then the end of the stream ----
  run(`globalThis.bv = document.createElement('video'); bv.src = URL.createObjectURL(new Blob([new Uint8Array([0x1a, 0x45, 0xdf, 0xa3, 7, 7, 7])]));`);
  await settle();
  out = take();
  const blobData = out.find((o) => o.data && o.id === run('bv.uid'));
  check(blobData && blobData.src === 0 && blobData.n === 7 && blobData.bytes[0] === 0x1a, 'a Blob src: its bytes go to source 0');
  check(!out.some((o) => o.cmd === 'eos ' + run('bv.uid') + ' 1'), 'no end of stream before the append is answered');
  media({ id: run('bv.uid'), buf: [0, blobData.seq, 0, [[0, 1 * S]]] });
  await settle();
  check(take().some((o) => o.cmd === 'eos ' + run('bv.uid') + ' 1'), 'end of stream once all is in');

  // ---- TimeRanges and Audio ----
  threw = false;
  try { run('mv.buffered.start(5)'); } catch (e) { threw = e.name === 'IndexSizeError'; }
  check(threw, 'TimeRanges index out of range throws');
  run(`globalThis.au = new Audio('/s.webm');`);
  await settle();
  out = take();
  check(run('au instanceof HTMLAudioElement') && out.some((o) => o.cmd === 'open ' + run('au.uid')), 'new Audio(src) loads');
  // A player the browser could not give.
  media({ id: run('au.uid'), fail: [4, 'too many players playing at once'] });
  await settle();
  check(run('au.error && au.error.code === 4'), 'no free player: MEDIA_ERR_SRC_NOT_SUPPORTED');

  // ---- what the browser measured, and the document while a video plays ----
  take();
  state(4, ['play', 'playing'], { rs: 4, p: 0, ct: 1 * S, w: 640, h: 360, d: 3 * S, fs: 50, fd: 10 });
  await settle();
  check(run(`const q = document.querySelector('video').getVideoPlaybackQuality(); q.totalVideoFrames === 60 && q.droppedVideoFrames === 10`),
    'getVideoPlaybackQuality: the pictures due (shown + dropped) and the dropped ones');
  media({ id: 0, cap: 2500000 });
  check(kjs.dom.decodeBudget === 2500000, 'the decoding budget (pixels a second) reaches the page');
  // What this machine plays smoothly: the parameters YouTube probes, then the measured budget.
  const vp9 = 'video/webm; codecs="vp9"';
  check(run(`MediaSource.isTypeSupported('${vp9}; width=1280; height=720; framerate=60') && MediaSource.isTypeSupported('${vp9}; width=1920; height=1080; framerate=60') && !MediaSource.isTypeSupported('${vp9}; width=3840; height=2160; framerate=30') && !MediaSource.isTypeSupported('${vp9}; width=1280; height=720; framerate=120')`),
    'isTypeSupported: up to 1920x1080 at 60 fps (what the decoder decodes at all)');
  const smooth = async (w, h, fps) => (await ctx.navigator.mediaCapabilities.decodingInfo({ type: 'media-source', video: { contentType: vp9, width: w, height: h, framerate: fps, bitrate: 1e6 } })).smooth;
  kjs.dom.decodeBudget = 3000000;
  check(await smooth(426, 240, 25) && !(await smooth(640, 360, 30)), 'decodingInfo: smooth within the budget (3 Mpixels/s, the emulator: 240p25 yes, 360p30 no)');
  kjs.dom.decodeBudget = 60000000;
  check(await smooth(1280, 720, 60) && !(await smooth(1920, 1080, 60)), 'decodingInfo: a fast machine (60 Mpixels/s) plays 720p60, not 1080p60');
  kjs.dom.decodeBudget = 0;
  check(await smooth(854, 480, 30) && !(await smooth(1280, 720, 30)) && !(await smooth(854, 480, 60)), 'decodingInfo before the first measure: up to 480p30');
  kjs.dom.decodeBudget = 2500000;
  const docs = () => take().filter((o) => o.other && o.other[0] === 'D').length;
  take();
  run(`document.body.setAttribute('data-a', '1')`);
  kjs.flush();
  const first = docs();
  run(`document.body.setAttribute('data-a', '2')`);
  kjs.flush();
  const second = docs();
  const wait = kjs.nextDelay();
  check(first === 1 && second === 0 && wait > 0 && wait <= 1000, 'while a video plays the document goes at most once a second');
  await new Promise((r) => setTimeout(r, 1050));
  kjs.flush();
  check(docs() === 1, 'the held document goes once the second is over');
  run(`document.body.setAttribute('data-a', '2')`);
  await new Promise((r) => setTimeout(r, 1050));
  kjs.flush();
  check(docs() === 0, 'a document equal to the last one sent is not sent again');

  console.log(failures ? failures + ' FAILED' : 'all passed');
  process.exit(failures ? 1 : 0);
})();
