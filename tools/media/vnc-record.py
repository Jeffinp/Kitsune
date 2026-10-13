#!/usr/bin/env python3
"""Record the guest's screen in real time through QEMU's own VNC server.

The `screendump` of the QEMU monitor gives ~4.6 pictures a second (each one is a PPM file written by
the monitor), so a demo built from it always looks stuck whatever the guest does. This is a minimal
RFB 3.8 client instead (no authentication, 32-bit pixels, Raw and CopyRect): it keeps one
incremental update request outstanding, applies every update to a copy of the screen and writes
that copy to ffmpeg through a pipe at a constant rate F, resampled by time (an output frame repeats
the last screen when nothing changed). Every update received is logged with its host time, so the
rate the guest really reached can be compared with the guest's own log (the truth: see
tools/media/frame-intervals.py).

    python3 tools/media/vnc-record.py --port 5907 --out /dev/shm/rec.mkv --fps 30 --secs 40 \
        --serial OUT/serial.log --start 'media: element [0-9]+ playing at' --delay 3 \
        --wav /dev/shm/audio.wav --log OUT/vnc

Writes OUT.updates.tsv (one line per update: host time since the start of the recording, rectangles,
pixels, bounding box), OUT.serial.tsv (the serial lines that matched --mark with the host time they
appeared), OUT.wav.tsv (the size of the WAV QEMU writes, sampled with the host time: it maps a sample
of the recording to a host time, which is how tools/media/vnc-mux.sh aligns the sound) and
OUT.summary.txt.

QEMU's VNC server looks for changes every 30 ms at most (GUI_REFRESH_INTERVAL_DEFAULT), so the
recording cannot show more than ~33 different pictures a second, and the times of the updates are
on that grid; a 25 fps video fits under it. QEMU's SDL window on the owner's machine has the same
30 ms refresh (GTK follows the monitor's refresh rate instead).
"""

import argparse
import os
import re
import socket
import struct
import subprocess
import sys
import threading
import time


def recv_exact(sock, n):
    buf = bytearray(n)
    view = memoryview(buf)
    got = 0
    while got < n:
        k = sock.recv_into(view[got:], n - got)
        if k == 0:
            raise EOFError("VNC server closed the connection")
        got += k
    return buf


class Screen:
    """The guest's screen as 32-bit B, G, R, X pixels."""

    def __init__(self, w, h):
        self.w, self.h = w, h
        self.fb = bytearray(w * h * 4)
        self.lock = threading.Lock()
        self.snapshot = bytes(self.fb)
        self.updates = 0

    def raw(self, x, y, w, h, data):
        stride = self.w * 4
        row = w * 4
        x = min(x, self.w)
        cw = max(0, min(w, self.w - x)) * 4
        for r in range(h):
            yy = y + r
            if yy >= self.h:
                break
            o = yy * stride + x * 4
            self.fb[o:o + cw] = data[r * row:r * row + cw]

    def copyrect(self, x, y, w, h, sx, sy):
        stride = self.w * 4
        rows = []
        for r in range(h):
            o = (sy + r) * stride + sx * 4
            rows.append(bytes(self.fb[o:o + w * 4]))
        for r, data in enumerate(rows):
            o = (y + r) * stride + x * 4
            self.fb[o:o + w * 4] = data

    def publish(self):
        snap = bytes(self.fb)
        with self.lock:
            self.snapshot = snap
            self.updates += 1


def handshake(sock):
    ver = recv_exact(sock, 12)
    if not ver.startswith(b"RFB "):
        raise RuntimeError("not an RFB server: %r" % bytes(ver))
    sock.sendall(b"RFB 003.008\n")
    n = recv_exact(sock, 1)[0]
    if n == 0:
        ln = struct.unpack(">I", recv_exact(sock, 4))[0]
        raise RuntimeError("VNC refused: %s" % recv_exact(sock, ln).decode(errors="replace"))
    types = recv_exact(sock, n)
    if 1 not in types:
        raise RuntimeError("the VNC server wants authentication (types %s)" % list(types))
    sock.sendall(b"\x01")
    res = struct.unpack(">I", recv_exact(sock, 4))[0]
    if res != 0:
        raise RuntimeError("VNC security handshake failed")
    sock.sendall(b"\x01")  # ClientInit: shared
    w, h = struct.unpack(">HH", recv_exact(sock, 4))
    recv_exact(sock, 16)  # the server's pixel format: replaced below
    ln = struct.unpack(">I", recv_exact(sock, 4))[0]
    name = recv_exact(sock, ln).decode(errors="replace")
    # SetPixelFormat: 32 bits, depth 24, little endian, true colour, R at bit 16, G 8, B 0: the bytes
    # are B, G, R, X (ffmpeg's bgr0).
    pf = struct.pack(">BBBBHHHBBBxxx", 32, 24, 0, 1, 255, 255, 255, 16, 8, 0)
    sock.sendall(b"\x00\x00\x00\x00" + pf)
    # SetEncodings: CopyRect (1), Raw (0), DesktopSize (-223).
    encs = [1, 0, -223]
    sock.sendall(struct.pack(">BxH", 2, len(encs)) + b"".join(struct.pack(">i", e) for e in encs))
    return w, h, name


def request(sock, w, h, incremental):
    sock.sendall(struct.pack(">BBHHHH", 3, 1 if incremental else 0, 0, 0, w, h))


class Recorder:
    def __init__(self, a):
        self.a = a
        self.stop = threading.Event()
        self.started = threading.Event()
        self.t0 = None  # host time (monotonic) of the first output frame
        self.mark_t = None  # host time the --start line appeared
        self.screen = None
        self.log = []
        self.serial_lines = []
        self.wav_sizes = []

    # ---- the VNC side ----

    def connect(self):
        deadline = time.monotonic() + self.a.wait_connect
        while True:
            try:
                s = socket.create_connection((self.a.host, self.a.port), timeout=5)
                s.settimeout(None)
                s.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                return s
            except OSError:
                if time.monotonic() > deadline:
                    raise
                time.sleep(0.5)

    def vnc_loop(self):
        sock = self.connect()
        w, h, name = handshake(sock)
        self.screen = Screen(w, h)
        print("vnc: %dx%d %r" % (w, h, name), flush=True)
        request(sock, w, h, False)
        while not self.stop.is_set():
            kind = recv_exact(sock, 1)[0]
            if kind == 0:
                (n,) = struct.unpack(">xH", recv_exact(sock, 3))
                area, box, rects = 0, None, 0
                for _ in range(n):
                    x, y, rw, rh, enc = struct.unpack(">HHHHi", recv_exact(sock, 12))
                    if enc == 0:
                        data = recv_exact(sock, rw * rh * 4)
                        self.screen.raw(x, y, rw, rh, data)
                    elif enc == 1:
                        sx, sy = struct.unpack(">HH", recv_exact(sock, 4))
                        self.screen.copyrect(x, y, rw, rh, sx, sy)
                    elif enc == -223:
                        # The guest changed its mode (the boot): a new screen of that size; the
                        # recording keeps the size it started with (see writer()).
                        if self.t0 is None:
                            self.screen = Screen(rw, rh)
                        w, h = rw, rh
                        continue
                    else:
                        raise RuntimeError("unexpected encoding %d" % enc)
                    rects += 1
                    area += rw * rh
                    b = (x, y, x + rw, y + rh)
                    box = b if box is None else (min(box[0], b[0]), min(box[1], b[1]),
                                                 max(box[2], b[2]), max(box[3], b[3]))
                # Ask for the next one before copying the screen: the server's 30 ms grid starts now.
                request(sock, w, h, True)
                self.screen.publish()
                now = time.monotonic()
                if self.t0 is not None and rects:
                    self.log.append((now - self.t0, rects, area, box))
            elif kind == 1:
                _, n = struct.unpack(">xHH", recv_exact(sock, 5))
                recv_exact(sock, n * 6)
            elif kind == 2:
                pass
            elif kind == 3:
                (ln,) = struct.unpack(">xxxI", recv_exact(sock, 7))
                recv_exact(sock, ln)
            else:
                raise RuntimeError("unexpected server message %d" % kind)
        sock.close()

    # ---- the clock of the recording ----

    def serial_loop(self):
        """Tail the serial log: the --start line starts the recording, --mark lines get host times."""
        pat = re.compile(self.a.start) if self.a.start else None
        mark = re.compile(self.a.mark) if self.a.mark else None
        f = None
        buf = b""
        while not self.stop.is_set():
            if f is None:
                try:
                    f = open(self.a.serial, "rb")
                    if self.a.serial_from_end:
                        f.seek(0, os.SEEK_END)
                except OSError:
                    time.sleep(0.05)
                    continue
            chunk = f.read()
            now = time.monotonic()
            if not chunk:
                time.sleep(0.005)
                continue
            buf += chunk
            *lines, buf = buf.split(b"\n")
            for raw in lines:
                line = raw.decode(errors="replace").rstrip("\r")
                if pat and self.mark_t is None and pat.search(line):
                    self.mark_t = now
                    print("vnc: start line seen: %s" % line, flush=True)
                    threading.Timer(self.a.delay, self.started.set).start()
                if mark and mark.search(line):
                    self.serial_lines.append((now, line))

    def wav_loop(self):
        """The size of the WAV QEMU writes, with the host time: its growth maps samples to time."""
        last = -1
        while not self.stop.is_set():
            try:
                size = os.path.getsize(self.a.wav)
            except OSError:
                size = -1
            now = time.monotonic()
            if size != last:
                self.wav_sizes.append((now, size))
                last = size
            time.sleep(0.005)

    # ---- the output ----

    def writer(self):
        self.started.wait()
        if self.stop.is_set():
            return
        scr = self.screen
        w, h = scr.w, scr.h
        cmd = ["ffmpeg", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "bgr0",
               "-s", "%dx%d" % (w, h), "-framerate", str(self.a.fps), "-i", "-",
               "-c:v", "libx264", "-preset", "ultrafast", "-crf", str(self.a.crf),
               "-pix_fmt", "yuv420p", self.a.out]
        # Low priority: the encoder must not take the host CPU QEMU's emulated CPU runs on.
        ff = subprocess.Popen(["nice", "-n", "15"] + cmd, stdin=subprocess.PIPE)
        period = 1.0 / self.a.fps
        self.t0 = time.monotonic()
        frames, late = 0, 0
        try:
            while not self.stop.is_set():
                due = self.t0 + frames * period
                now = time.monotonic()
                if due > now:
                    time.sleep(due - now)
                elif now - due > period:
                    late += 1
                with self.screen.lock:
                    snap = self.screen.snapshot
                if len(snap) != w * h * 4:
                    snap = bytes(w * h * 4)
                ff.stdin.write(snap)
                frames += 1
                if frames >= self.a.secs * self.a.fps:
                    break
        finally:
            ff.stdin.close()
            ff.wait()
            self.frames, self.late = frames, late
            self.stop.set()

    def run(self):
        threads = [threading.Thread(target=self.vnc_loop, daemon=True),
                   threading.Thread(target=self.wav_loop, daemon=True)]
        if self.a.serial:
            threads.append(threading.Thread(target=self.serial_loop, daemon=True))
        else:
            self.mark_t = time.monotonic()
            threading.Timer(self.a.delay, self.started.set).start()
        for t in threads:
            t.start()
        wt = threading.Thread(target=self.writer)
        wt.start()
        if self.a.wait_start:
            if not self.started.wait(self.a.wait_start):
                print("vnc: the start line never came", flush=True)
                self.stop.set()
                self.started.set()
        wt.join()
        self.stop.set()
        self.report()

    def report(self):
        base = self.a.log
        if self.t0 is None:
            return
        with open(base + ".updates.tsv", "w") as f:
            f.write("# t_s\trects\tpixels\tx0\ty0\tx1\ty1\n")
            for t, n, area, box in self.log:
                f.write("%.4f\t%d\t%d\t%d\t%d\t%d\t%d\n" % ((t, n, area) + tuple(box)))
        with open(base + ".serial.tsv", "w") as f:
            for t, line in self.serial_lines:
                f.write("%.4f\t%s\n" % (t - self.t0, line))
        with open(base + ".wav.tsv", "w") as f:
            f.write("# t_s\tbytes\n")
            for t, size in self.wav_sizes:
                f.write("%.4f\t%d\n" % (t - self.t0, size))
        # Updates that touched the video box (the biggest rectangle that keeps coming back) give
        # the rate of pictures the recording shows.
        secs = self.frames / self.a.fps
        ts = [t for t, *_ in self.log if 0 <= t <= secs]
        big = [t for t, n, area, box in self.log if 0 <= t <= secs and area >= self.a.min_area]
        lines = ["recorded %d frames at %s fps (%.1f s), %d written late" %
                 (self.frames, self.a.fps, secs, self.late),
                 "updates from the VNC server: %d (%.1f/s)" % (len(ts), len(ts) / max(secs, 1e-9)),
                 "updates of at least %d pixels (the video): %d (%.1f/s)" %
                 (self.a.min_area, len(big), len(big) / max(secs, 1e-9))]
        if len(big) > 2:
            gaps = sorted((b - a) * 1000 for a, b in zip(big, big[1:]))
            mean = sum(gaps) / len(gaps)
            sd = (sum((g - mean) ** 2 for g in gaps) / len(gaps)) ** 0.5
            pct = lambda p: gaps[min(len(gaps) - 1, int(p / 100 * len(gaps)))]
            lines.append("intervals between video updates (ms): mean %.1f sd %.1f p50 %.1f p95 %.1f max %.1f"
                         % (mean, sd, pct(50), pct(95), gaps[-1]))
        if self.mark_t is not None:
            lines.append("start line at %.3f s before the first frame" % (self.t0 - self.mark_t))
        text = "\n".join(lines)
        print(text, flush=True)
        with open(base + ".summary.txt", "w") as f:
            f.write(text + "\n")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--host", default="127.0.0.1")
    p.add_argument("--port", type=int, default=5907)
    p.add_argument("--out", required=True, help="the video (any container ffmpeg writes; .mkv is safest)")
    p.add_argument("--fps", type=int, default=30, help="output frame rate (resampled by time)")
    p.add_argument("--secs", type=float, default=40)
    p.add_argument("--crf", type=int, default=18)
    p.add_argument("--serial", help="the guest's serial log (QEMU -serial file:...)")
    p.add_argument("--serial-from-end", action="store_true", help="ignore what the serial log already has")
    p.add_argument("--start", help="regex of the serial line that starts the recording")
    p.add_argument("--delay", type=float, default=0, help="seconds between the start line and the recording")
    p.add_argument("--wait-start", type=float, default=0, help="give up after this many seconds without it")
    p.add_argument("--mark", default=r"media: element", help="regex of serial lines to time")
    p.add_argument("--wav", default="/nonexistent", help="the WAV QEMU writes (-audiodev wav,path=...)")
    p.add_argument("--wait-connect", type=float, default=600)
    p.add_argument("--min-area", type=int, default=40000, help="pixels of an update that counts as the video")
    p.add_argument("--log", help="prefix of the logs (default: --out without its extension)")
    a = p.parse_args()
    if not a.log:
        a.log = os.path.splitext(a.out)[0]
    Recorder(a).run()


if __name__ == "__main__":
    sys.exit(main())
