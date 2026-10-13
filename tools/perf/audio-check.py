#!/usr/bin/env python3
"""Check what the guest played, from the WAV QEMU recorded (`-audiodev wav,...`).

    audio-check.py CAPTURE.wav PLAN [--serial serial.log]

PLAN lists the sounds expected, in order, separated by `;`. Each is
`FREQ[+FREQ...]:MS[:REL]` (REL: the expected amplitude relative to the first sound, e.g. 0.25).
Between two sounds, and after the last, there must be silence (exact zeros: the mixer outputs zero
and the AC'97 device halts on silence). For each sound the script measures the frequency (FFT with
a Hann window and a parabolic peak, or zero crossings without numpy), the duration (first to last
sample above the noise floor), the amplitude (RMS of the middle half) and how much of the energy is
at the expected frequencies. With --serial it also checks the stream clock lines that
`play --clock` printed (`clock uptime_ms=... played_ms=...`): the clock must advance at the speed of
the system clock.

Prints one PASS/MISS line per check and `VERDICT: PASS|FAIL` at the end.
"""
import math
import re
import sys
import wave

try:
    import numpy as np
except ImportError:  # the zero-crossing fallback measures single tones only
    np = None

FLOOR = 40          # |sample| above this is sound (the tones peak near 12000)
MIN_GAP_MS = 150    # silence that separates two sounds
FREQ_TOL = 1.5      # Hz
DUR_TOL_MS = 12
REL_TOL = 0.04


def read(path):
    w = wave.open(path)
    rate, ch, width, n = w.getframerate(), w.getnchannels(), w.getsampwidth(), w.getnframes()
    raw = w.readframes(n)
    if n == 0:
        # QEMU writes the sizes in the header only when it quits: read what is there.
        with open(path, "rb") as f:
            raw = f.read()[44:]
        raw = raw[: len(raw) - len(raw) % (ch * width)]
    if width != 2:
        sys.exit(f"{path}: {width * 8}-bit capture, expected 16")
    if np is not None:
        x = np.frombuffer(raw, dtype="<i2").reshape(-1, ch).astype(float)
        return rate, x[:, 0], (x[:, 1] if ch > 1 else x[:, 0])
    import array
    a = array.array("h", raw)
    return rate, a[0::ch], a[1::ch] if ch > 1 else a[0::ch]


def segments(left, rate):
    """(start, end) frame spans of sound separated by at least MIN_GAP_MS of near-silence."""
    gap = rate * MIN_GAP_MS // 1000
    spans, start, last = [], None, None
    for i, v in enumerate(left):
        if abs(v) > FLOOR:
            if start is None:
                start = i
            elif i - last > gap:
                spans.append((start, last + 1))
                start = i
            last = i
    if start is not None:
        spans.append((start, last + 1))
    return spans


def peaks(seg, rate, count):
    """The `count` strongest frequencies (Hz) and the share of energy within 8 Hz of them."""
    if np is None:
        crossings = sum(1 for a, b in zip(seg, seg[1:]) if a < 0 <= b)
        return [crossings * rate / len(seg)], None
    win = np.hanning(len(seg))
    spec = np.abs(np.fft.rfft(seg * win))
    freqs = np.fft.rfftfreq(len(seg), 1 / rate)
    found, s = [], spec.copy()
    for _ in range(count):
        k = int(np.argmax(s))
        if 0 < k < len(s) - 1:
            a, b, c = spec[k - 1], spec[k], spec[k + 1]
            d = 0.5 * (a - c) / (a - 2 * b + c) if (a - 2 * b + c) != 0 else 0
        else:
            d = 0
        found.append((k + d) * rate / len(seg))
        lo, hi = max(0, k - 20), k + 21
        s[lo:hi] = 0
    power = spec ** 2
    near = np.zeros_like(power, dtype=bool)
    for f in found:
        near |= np.abs(freqs - f) <= 8
    return sorted(found), float(power[near].sum() / power.sum())


def rms(seg):
    if np is not None:
        return float(np.sqrt(np.mean(seg ** 2)))
    return math.sqrt(sum(v * v for v in seg) / max(1, len(seg)))


def main():
    args = sys.argv[1:]
    serial = None
    if "--serial" in args:
        i = args.index("--serial")
        serial = args[i + 1]
        del args[i:i + 2]
    path, plan = args[0], [p for p in args[1].split(";") if p]
    rate, left, right = read(path)
    spans = segments(left, rate)
    ok = True

    def check(cond, what):
        nonlocal ok
        print(("PASS " if cond else "MISS ") + what)
        ok &= bool(cond)

    print(f"capture: {path}: {rate} Hz, {len(left) / rate:.2f} s, {len(spans)} sound(s)")
    check(len(spans) == len(plan), f"{len(plan)} sounds expected, {len(spans)} found")
    ref = None
    for n, (want, (a, b)) in enumerate(zip(plan, spans), 1):
        parts = want.split(":")
        fwant = [float(f) for f in parts[0].split("+")]
        dwant = float(parts[1])
        rwant = float(parts[2]) if len(parts) > 2 else None
        seg = left[a:b]
        dur = (b - a) * 1000 / rate
        mid = seg[len(seg) // 4: 3 * len(seg) // 4]
        level = rms(mid)
        ref = ref or level
        got, purity = peaks(seg, rate, len(fwant))
        gap_after = ((spans[n][0] if n < len(spans) else len(left)) - b) * 1000 / rate
        freqs = " + ".join(f"{f:.2f}" for f in got)
        pur = f", {purity * 100:.2f}% of the energy there" if purity is not None else ""
        print(f"  sound {n}: at {a / rate:.3f} s, {freqs} Hz, {dur:.1f} ms, rms {level:.0f}"
              f" ({level / ref:.3f} of the first){pur}, then {gap_after:.0f} ms of silence")
        check(len(got) == len(fwant) and all(abs(g - w) <= FREQ_TOL for g, w in zip(got, sorted(fwant))),
              f"sound {n}: frequency {'+'.join(parts[0].split('+'))} Hz (got {freqs})")
        check(abs(dur - dwant) <= DUR_TOL_MS, f"sound {n}: duration {dwant:.0f} ms (got {dur:.1f})")
        if purity is not None:
            check(purity > 0.99, f"sound {n}: clean (> 99% of the energy at the tone)")
        if rwant is not None:
            check(abs(level / ref - rwant) <= REL_TOL, f"sound {n}: level {rwant} of the first (got {level / ref:.3f})")
        silent = all(v == 0 for v in left[b + rate // 100: b + rate * min(int(gap_after), 1000) // 1000])
        check(gap_after >= 300 and silent, f"sound {n}: silence after it ({gap_after:.0f} ms, exact zeros)")
        if np is not None:
            same = float(np.max(np.abs(seg - right[a:b]))) if len(seg) else 0
            check(same <= 1, f"sound {n}: both channels the same (max difference {same:.0f})")
    if serial:
        pts = []
        for line in open(serial, errors="replace"):
            m = re.search(r"clock uptime_ms=(\d+) played_ms=(\d+)", line)
            if m:
                pts.append((int(m.group(1)), int(m.group(2))))
        moving = [(u, p) for u, p in pts if p > 0]
        if len(moving) >= 3:
            (u0, p0), (u1, p1) = moving[0], moving[-1]
            slope = (p1 - p0) / max(1, u1 - u0)
            print(f"  clock: {len(pts)} readings, played {p0}..{p1} ms over uptime {u0}..{u1} ms")
            check(abs(slope - 1) <= 0.05, f"stream clock runs at the system clock's speed (slope {slope:.3f})")
            mono = all(b[1] >= a[1] for a, b in zip(pts, pts[1:]))
            check(mono, "stream clock never goes back")
        else:
            check(False, f"clock readings in {serial} ({len(pts)} found)")
    print("VERDICT: PASS" if ok else "VERDICT: FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
