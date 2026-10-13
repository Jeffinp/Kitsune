#!/usr/bin/env python3
"""Put the sound QEMU recorded under a recording of tools/media/vnc-record.py.

    python3 tools/media/vnc-mux.py REC.mkv AUDIO.wav OUT.mp4 [--log PREFIX]

QEMU's `-audiodev wav` writes the samples the emulated AC'97 plays, as it plays them (in host time),
and vnc-record.py sampled the size of that file with the host time while it recorded
(PREFIX.wav.tsv). A straight line fitted through those points says which sample was being played
when the recording started; the sound is cut from there. The file grows in pieces of the C library's
buffer (4 KiB = 21 ms of 48 kHz stereo) and the sampling looks every 5 ms, so a point lags the
truth by 0-26 ms: half of that is added back, and the fit's residual is printed. What is left is the
alignment error of the recording (a few ms to ~15 ms), well under a frame of a 25 fps video.
"""

import argparse
import os
import subprocess
import sys
import wave

RATE = 48000
FRAME = 4  # stereo s16
HEADER = 44


def fit(points):
    n = len(points)
    mx = sum(t for t, _ in points) / n
    my = sum(f for _, f in points) / n
    sxx = sum((t - mx) ** 2 for t, _ in points)
    sxy = sum((t - mx) * (f - my) for t, f in points)
    slope = sxy / sxx if sxx else RATE
    return slope, my - slope * mx


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("video")
    p.add_argument("wav")
    p.add_argument("out")
    p.add_argument("--log", help="the prefix vnc-record.py used (default: the video without its extension)")
    p.add_argument("--crf", type=int, default=23)
    p.add_argument("--extra-ms", type=float, default=0, help="move the sound later by this much")
    a = p.parse_args()
    log = a.log or os.path.splitext(a.video)[0]
    pts = []
    for line in open(log + ".wav.tsv"):
        if line.startswith("#"):
            continue
        t, size = line.split()
        t, size = float(t), int(size)
        if size > HEADER:
            pts.append((t, (size - HEADER) / FRAME))
    with wave.open(a.wav) as w:
        total = w.getnframes() / w.getframerate()
    # Only the points while the recording ran and the file grew steadily (a pause of the sound stops
    # the growth; those points do not fit a line).
    rec = [(t, f) for t, f in pts if t >= -1.0]
    if len(rec) < 4:
        print("vnc-mux: the WAV did not grow during the recording; the sound starts at 0")
        start = 0.0
        resid = None
        slope = RATE
    else:
        slope, icept = fit(rec)
        # Lag of a sampled size behind what was played: half of a 4 KiB buffer plus half of a 5 ms poll.
        lag = (4096 / FRAME) / 2 + RATE * 0.0025
        start = (icept + lag) / RATE
        resid = max(abs(f - (slope * t + icept)) for t, f in rec) / RATE * 1000
    start += a.extra_ms / 1000
    print("vnc-mux: WAV of %.1f s; %d size samples; rate %.1f frames/s (nominal %d); "
          "the recording starts %.3f s into it%s" %
          (total, len(rec), slope, RATE, start,
           "" if resid is None else "; fit residual max %.1f ms" % resid))
    if start < 0:
        cmd_audio = ["-itsoffset", "%.4f" % -start, "-i", a.wav]
    else:
        cmd_audio = ["-ss", "%.4f" % start, "-i", a.wav]
    cmd = ["ffmpeg", "-loglevel", "error", "-y", "-i", a.video] + cmd_audio + [
        "-map", "0:v", "-map", "1:a", "-c:v", "libx264", "-preset", "medium", "-crf", str(a.crf),
        "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "128k", "-shortest", "-movflags", "+faststart",
        a.out]
    subprocess.run(cmd, check=True)
    print("vnc-mux: wrote %s (%d bytes)" % (a.out, os.path.getsize(a.out)))


if __name__ == "__main__":
    sys.exit(main())
