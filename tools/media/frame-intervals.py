#!/usr/bin/env python3
"""The intervals between the video pictures the guest put on screen, from its own log.

    python3 tools/media/frame-intervals.py OUT/serial.log [--skip S] [--secs S] [--element N] [--tsv FILE]

The browser logs, once a second while a video plays, when each picture reached the screen (the TSC
read right after the compositor uploaded it), its time in the video and how late it was against the
clock (kernel/src/desktop/apps/browser/media/stats.rs, kitsune_core::media::pacing::FrameTimes):

    media: element 1 frames at 52340.1 pts 10040: 40.0/40/3 40.1/40/2 ...

(interval since the picture before, ms / step of the video time, ms / lateness, ms). This prints,
per element, the frame rate shown, the spread of the intervals (mean, standard deviation, min,
percentiles, max), the hitches (intervals over 1.5 frame durations), the steps of the video time
(1 frame, or more when pictures were dropped) and the lateness, over the whole playback (after
--skip seconds) and per 10 s; then the guest's own 10-second lines (fps, dropped, CPU) and the
`waiting` count. This is the measure of record: a screen capture only samples it.
"""

import argparse
import re
import sys
from collections import Counter, defaultdict

LINE = re.compile(r"media: element (\d+) frames at ([\d.]+) pts (-?\d+):(.*)$")


def spread(v):
    v = sorted(v)
    n = len(v)
    if not n:
        return None
    mean = sum(v) / n
    sd = (sum((x - mean) ** 2 for x in v) / n) ** 0.5
    pct = lambda p: v[max(0, min(n - 1, -(-p * n // 100) - 1))]
    return dict(n=n, mean=mean, sd=sd, min=v[0], p50=pct(50), p95=pct(95), p99=pct(99), max=v[-1])


def fmt(s):
    return ("n %d  mean %.1f  sd %.1f  min %.1f  p50 %.1f  p95 %.1f  p99 %.1f  max %.1f" %
            (s["n"], s["mean"], s["sd"], s["min"], s["p50"], s["p95"], s["p99"], s["max"]))


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("serial")
    p.add_argument("--skip", type=float, default=2.0, help="seconds of playback to leave out at the start")
    p.add_argument("--secs", type=float, default=0, help="only this many seconds after the skip (0: all)")
    p.add_argument("--element", type=int)
    p.add_argument("--tsv", help="write every picture (element, at ms, pts ms, interval ms, late ms)")
    a = p.parse_args()
    frames = defaultdict(list)  # element -> [(at_ms, pts_ms, interval_ms or None, late_ms)]
    other = []
    waiting = Counter()
    with open(a.serial, "rb") as f:
        for raw in f:
            line = raw.decode(errors="replace").strip()
            m = LINE.search(line)
            if m:
                el = int(m.group(1))
                at, pts = float(m.group(2)), int(m.group(3))
                fl = frames[el]
                if not fl or abs(fl[-1][0] - at) > 0.15:
                    fl.append((at, pts, None, 0))  # a new run starts (no interval across it)
                t, q = at, pts
                for e in m.group(4).split():
                    # Another thread's output can land inside the line (the serial port is shared):
                    # the rest of the line is lost, and the next line starts a new run.
                    try:
                        dt, dp, late = e.split("/")
                        dt, dp, late = float(dt), int(dp), int(late)
                    except ValueError:
                        break
                    t += dt
                    q += dp
                    fl.append((t, q, dt, late))
                continue
            if "media: element" in line and " played " in line:
                other.append(line[line.index("media: element"):])
                if "(waiting)" in line:
                    waiting[int(line.split()[2])] += 1
    if not frames:
        print("no 'media: element N frames' lines: an image without the frame log?")
        for o in other:
            print(o)
        return 1
    tsv = open(a.tsv, "w") if a.tsv else None
    for el, fl in sorted(frames.items()):
        if a.element is not None and el != a.element:
            continue
        start = fl[0][0] + a.skip * 1000
        end = start + a.secs * 1000 if a.secs else float("inf")
        sel = [f for f in fl if start <= f[0] <= end]
        if tsv:
            for f in fl:
                tsv.write("%d\t%.1f\t%d\t%s\t%d\n" % (el, f[0], f[1], "" if f[2] is None else "%.1f" % f[2], f[3]))
        ivs = [f[2] for f in sel if f[2] is not None]
        steps = [b[1] - a_[1] for a_, b in zip(sel, sel[1:]) if b[2] is not None]
        if not ivs:
            continue
        dur = (sel[-1][0] - sel[0][0]) / 1000
        nominal = Counter(steps).most_common(1)[0][0] if steps else 0
        s = spread(ivs)
        print("element %d: %d pictures in %.1f s = %.2f fps shown; video frame %d ms (%.2f fps)" %
              (el, len(sel), dur, (len(sel) - 1) / dur if dur else 0, nominal,
               1000 / nominal if nominal else 0))
        print("  intervals (ms): " + fmt(s))
        hitch = [x for x in ivs if nominal and x > 1.5 * nominal]
        print("  hitches (> 1.5 frames): %d (%.1f%%); worst %.1f ms" %
              (len(hitch), 100 * len(hitch) / len(ivs), max(ivs)))
        sc = Counter(steps)
        print("  video-time steps: " + ", ".join("%d ms x%d" % (k, v) for k, v in sorted(sc.items())))
        lates = [f[3] for f in sel]
        print("  lateness against the clock (ms): mean %.1f, max %d" % (sum(lates) / len(lates), max(lates)))
        # Per 10 s window.
        t0 = sel[0][0]
        win = defaultdict(list)
        for f in sel:
            if f[2] is not None:
                win[int((f[0] - t0) // 10000)].append(f[2])
        for k in sorted(win):
            w = spread(win[k])
            print("  %3d-%3d s: %5.1f fps  %s" % (k * 10, k * 10 + 10, 1000 / w["mean"], fmt(w)))
    print("guest 10 s lines:")
    for o in other:
        print("  " + o)
    print("waiting: " + (", ".join("element %d x%d" % kv for kv in waiting.items()) or "0"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
