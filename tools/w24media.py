"""The routes of tools/w24-site.py for <video>, <audio> and Media Source Extensions (W58).

/video     <video controls src=/media/v360.webm>: a 3 s 640x360 VP9 video with a 440 Hz Opus tone
           (made with the host's ffmpeg). Its script plays it when it can, logs the events
           (`media-test: ...` on the guest's serial line), seeks back to 1 s after the end and plays
           to the end again, then says DONE.
/mse       the same media through Media Source Extensions: video and sound in two WebM DASH files
           (/media/msev.webm, /media/msea.webm), the init segment and each cluster fetched with a
           Range request and appended to two SourceBuffers (/media/mse.json lists the byte ranges).
/audio     <audio controls src=/media/a440.webm>: the tone alone.
/media/*   the files, with Range (206) support.
/real?src=URL  a page with `<video controls src=URL>` and the same script (plays it once, logs the
           events): for a video from the internet, or /relay?u=URL, the same file downloaded once by
           this server (with the host's curl, through its proxy) and served with Range, for a
           sandbox where the guest cannot verify the real site's certificate.

The files are made once in $W24_MEDIA (default /tmp/w24media). `python3 tools/w24media.py DIR` makes
them in DIR and prints the index.
"""
import json
import os
import subprocess
import sys
from urllib.parse import parse_qs

MEDIA = os.environ.get("W24_MEDIA", "/tmp/w24media")
_index = None


def make(d=None):
    """Make the test files (once) and return the MSE index."""
    global _index
    d = d or MEDIA
    os.makedirs(d, exist_ok=True)
    ff = ["ffmpeg", "-v", "error", "-y"]
    v360 = os.path.join(d, "v360.webm")
    if not os.path.exists(v360):
        subprocess.run(ff + ["-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30", "-f", "lavfi",
                             "-i", "sine=frequency=440:sample_rate=48000", "-t", "3", "-c:v", "libvpx-vp9",
                             "-b:v", "300k", "-g", "30", "-c:a", "libopus", v360], check=True)
    for name, sel in (("msev.webm", "0:v"), ("msea.webm", "0:a")):
        p = os.path.join(d, name)
        if not os.path.exists(p):
            subprocess.run(ff + ["-i", v360, "-map", sel, "-c", "copy", "-f", "webm", "-dash", "1",
                                 "-cluster_time_limit", "1000", p], check=True)
    a440 = os.path.join(d, "a440.webm")
    if not os.path.exists(a440):
        subprocess.run(ff + ["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000", "-t", "2",
                             "-c:a", "libopus", a440], check=True)
    if _index is None or d != MEDIA:
        _index = {
            "duration": 3.0,
            "video": dict(url="/media/msev.webm", type='video/webm; codecs="vp9"', **clusters(os.path.join(d, "msev.webm"))),
            "audio": dict(url="/media/msea.webm", type='audio/webm; codecs="opus"', **clusters(os.path.join(d, "msea.webm"))),
        }
    return _index


def vint(b, i, keep_marker):
    """An EBML variable-size integer at b[i]: (value, length)."""
    first = b[i]
    n = 1
    while n <= 8 and not first & (0x80 >> (n - 1)):
        n += 1
    v = first if keep_marker else first & (0xFF >> n)
    for k in range(1, n):
        v = (v << 8) | b[i + k]
    unknown = not keep_marker and v == (1 << (7 * n)) - 1
    return (None if unknown else v), n


def clusters(path):
    """Byte ranges of the init segment (EBML header .. first cluster) and of each cluster."""
    b = open(path, "rb").read()
    i = 0
    eid, n = vint(b, i, True)
    size, m = vint(b, i + n, False)
    i += n + m + size                      # the EBML header
    eid, n = vint(b, i, True)
    assert eid == 0x18538067, "no Segment"
    size, m = vint(b, i + n, False)
    i += n + m
    end = len(b) if size is None else min(len(b), i + size)
    first, segs = None, []
    while i < end:
        eid, n = vint(b, i, True)
        size, m = vint(b, i + n, False)
        if size is None:
            size = end - (i + n + m)
        if eid == 0x1F43B675:              # Cluster
            if first is None:
                first = i
            segs.append([i, i + n + m + size])
        i += n + m + size
    return {"init": [0, first], "segs": segs}


def page(title, body):
    return ("<!doctype html><html lang=pt-BR><head><meta charset=utf-8><title>" + title +
            "</title><style>body{font-family:sans-serif;margin:16px}video{display:block;margin:8px 0}"
            "p{margin:4px 0}</style></head><body>" + body + "</body></html>").encode()


# The script both pages share: log the events, play when ready, seek back after the end.
LOG_JS = r"""
const v = document.getElementById('v');
const t0 = Date.now();
const log = (m) => console.log('media-test: ' + m + ' (' + (Date.now() - t0) + ' ms)');
const show = (m) => { document.getElementById('s').textContent = m; };
for (const e of ['loadstart', 'loadedmetadata', 'loadeddata', 'canplay', 'canplaythrough', 'play', 'playing',
  'pause', 'waiting', 'seeking', 'seeked', 'ended', 'error', 'durationchange', 'emptied'])
  v.addEventListener(e, () => log(e + ' t=' + v.currentTime.toFixed(3) + ' rs=' + v.readyState + ' d=' + v.duration +
    (e === 'error' && v.error ? ' code=' + v.error.code + ' ' + v.error.message : '')));
let tu = 0;
v.addEventListener('timeupdate', () => { if (++tu % 4 === 0) log('timeupdate ' + v.currentTime.toFixed(2)); });
let round = 0;
const start = () => {
  if (round !== 0) return;
  round = 1;
  log('size ' + v.videoWidth + 'x' + v.videoHeight + ' buffered ' + (v.buffered.length ? v.buffered.end(v.buffered.length - 1).toFixed(2) : 0));
  v.play().then(() => log('play resolved'), (e) => log('play rejected ' + e));
};
v.addEventListener('ended', () => {
  if (round === 1) {
    round = 2;
    log('first end at ' + v.currentTime.toFixed(2));
    show('primeira volta');
    setTimeout(() => { v.currentTime = 1; v.play(); }, 700);
  } else if (round === 2) {
    round = 3;
    log('DONE played ' + v.played.length + ' range(s), ended ' + v.ended + ' paused ' + v.paused);
    show('fim');
  } else log('ended again');
});
"""

VIDEO_PAGE = page("Vídeo", """<h1>Vídeo</h1>
<video id=v controls src=/media/v360.webm></video>
<p id=s>carregando</p>
<script>""" + LOG_JS + """
v.addEventListener('canplaythrough', start);
log('canPlayType ' + v.canPlayType('video/webm; codecs="vp9, opus"'));
</script>""")

AUDIO_PAGE = page("Áudio", """<h1>Áudio</h1>
<audio id=v controls src=/media/a440.webm></audio>
<p id=s>carregando</p>
<script>""" + LOG_JS + """
v.addEventListener('canplaythrough', start);
</script>""")

MSE_PAGE = page("MSE", """<h1>Media Source Extensions</h1>
<video id=v controls width=640 height=360></video>
<p id=s>carregando</p>
<script>""" + LOG_JS + r"""
log('MediaSource ' + typeof MediaSource + ' ' + MediaSource.isTypeSupported('video/webm; codecs="vp9"'));
const ms = new MediaSource();
v.src = URL.createObjectURL(ms);
const get = async (url, [a, b]) => {
  const r = await fetch(url, { headers: { Range: 'bytes=' + a + '-' + (b - 1) } });
  if (r.status !== 206) throw new Error('status ' + r.status);
  return new Uint8Array(await r.arrayBuffer());
};
const append = (sb, data) => new Promise((ok, fail) => {
  const done = () => { sb.removeEventListener('error', bad); ok(); };
  const bad = () => { sb.removeEventListener('updateend', done); fail(new Error('append error')); };
  sb.addEventListener('updateend', done, { once: true });
  sb.addEventListener('error', bad, { once: true });
  sb.appendBuffer(data);
});
ms.addEventListener('sourceopen', async () => {
  try {
    log('sourceopen ' + ms.readyState);
    const idx = await (await fetch('/media/mse.json')).json();
    ms.duration = idx.duration;
    const vb = ms.addSourceBuffer(idx.video.type);
    const ab = ms.addSourceBuffer(idx.audio.type);
    await append(vb, await get(idx.video.url, idx.video.init));
    await append(ab, await get(idx.audio.url, idx.audio.init));
    const n = Math.max(idx.video.segs.length, idx.audio.segs.length);
    for (let i = 0; i < n; i++) {
      if (idx.video.segs[i]) await append(vb, await get(idx.video.url, idx.video.segs[i]));
      if (idx.audio.segs[i]) await append(ab, await get(idx.audio.url, idx.audio.segs[i]));
      log('segment ' + i + ' video ' + vb.buffered.end(0).toFixed(2) + ' audio ' + ab.buffered.end(0).toFixed(2) + ' element ' + v.buffered.end(0).toFixed(2));
    }
    ms.endOfStream();
    log('endOfStream ' + ms.readyState + ' duration ' + ms.duration.toFixed(3) + ' segments ' + idx.video.segs.length + '+' + idx.audio.segs.length);
    start();
  } catch (e) { log('MSE failed: ' + e); }
});
ms.addEventListener('sourceended', () => log('sourceended'));
</script>""")


REAL_JS = r"""
v.addEventListener('canplaythrough', start);
v.addEventListener('loadedmetadata', () => log('metadata ' + v.videoWidth + 'x' + v.videoHeight + ' duration ' + v.duration));
"""


def real_page(src):
    import html
    return page("Vídeo real", "<h1>Vídeo da internet</h1><video id=v controls src=\"" + html.escape(src) +
                "\"></video><p id=s>carregando</p><p>" + html.escape(src) + "</p><script>" + LOG_JS + REAL_JS +
                "</script>")


def relay_file(url):
    """The file at `url`, downloaded once into MEDIA (None if it failed)."""
    import hashlib
    os.makedirs(MEDIA, exist_ok=True)
    p = os.path.join(MEDIA, "relay-" + hashlib.sha1(url.encode()).hexdigest()[:16] + ".webm")
    if not os.path.exists(p):
        r = subprocess.run(["curl", "-sS", "-f", "-m", "120", "-A", "Mozilla/5.0 KitsuneTest", "-o", p + ".part", url])
        if r.returncode != 0:
            return None
        os.rename(p + ".part", p)
    return p


def serve_file(h, p):
    data = open(p, "rb").read()
    n = len(data)
    rng = h.headers.get("Range", "")
    if rng.startswith("bytes="):
        a, _, b = rng[6:].partition("-")
        a = int(a or 0)
        b = min(int(b) if b else n - 1, n - 1)
        if a >= n:
            h.reply(416, "text/plain", b"", [("Content-Range", f"bytes */{n}")])
            return
        h.reply(206, "video/webm", data[a:b + 1], [("Content-Range", f"bytes {a}-{b}/{n}"),
                                                  ("Accept-Ranges", "bytes")])
        return
    h.reply(200, "video/webm", data, [("Accept-Ranges", "bytes")])


def handle(h, method, u, q):
    """Serve a media route; False if the path is not one."""
    if method not in ("GET", "HEAD"):
        return False
    if u.path == "/real":
        h.reply(200, "text/html; charset=utf-8", real_page(q.get("src", [""])[0]))
        return True
    if u.path == "/relay":
        p = relay_file(q.get("u", [""])[0])
        if p is None:
            h.reply(502, "text/plain", b"the download failed")
        else:
            serve_file(h, p)
        return True
    if u.path == "/video":
        h.reply(200, "text/html; charset=utf-8", VIDEO_PAGE)
        return True
    if u.path == "/audio":
        h.reply(200, "text/html; charset=utf-8", AUDIO_PAGE)
        return True
    if u.path == "/mse":
        h.reply(200, "text/html; charset=utf-8", MSE_PAGE)
        return True
    if u.path == "/media/mse.json":
        h.reply(200, "application/json", json.dumps(make()).encode())
        return True
    if u.path.startswith("/media/") and u.path.endswith(".webm"):
        make()
        name = os.path.basename(u.path)
        p = os.path.join(MEDIA, name)
        if not os.path.isfile(p):
            h.reply(404, "text/plain", b"no such file")
            return True
        serve_file(h, p)
        return True
    return False


if __name__ == "__main__":
    print(json.dumps(make(sys.argv[1] if len(sys.argv) > 1 else None), indent=1))
