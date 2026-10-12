#!/usr/bin/env python3
"""render-page.py PAGE.html OUT.png [width] [max_height] [--css FILE...]

Lays PAGE.html out with the browser engine (kitsune_core example `render_page`) and paints the display
list with PIL, so a real page can be inspected on the host without booting the system. Text uses
DejaVu Sans, so widths differ a little from the kernel's Inter face; the structure is the engine's."""
import subprocess, sys
from PIL import Image, ImageDraw, ImageFont

argv = sys.argv[1:]
css = []
if "--css" in argv:
    i = argv.index("--css")
    css = ["--css"] + argv[i + 1:]
    argv = argv[:i]
page, out = argv[0], argv[1]
width = int(argv[2]) if len(argv) > 2 else 960
maxh = int(argv[3]) if len(argv) > 3 else 3000
lines = subprocess.run(
    ["cargo", "run", "-q", "--release", "-p", "kitsune_core", "--example", "render_page", "--", page, str(width), "100"] + css,
    capture_output=True, text=True, check=True).stdout.split("\n")
head = lines[0].split("\t")
h = min(int(head[2]), maxh)
img = Image.new("RGB", (width, max(h, 1)), tuple(int(v) for v in head[3:6]))
d = ImageDraw.Draw(img)
fonts = {}
def font(size, bold, mono):
    k = (size, bold, mono)
    if k not in fonts:
        n = "DejaVuSansMono" if mono else "DejaVuSans"
        n += "-Bold" if bold else ""
        fonts[k] = ImageFont.truetype(f"/usr/share/fonts/truetype/dejavu/{n}.ttf", max(size, 4))
    return fonts[k]
for ln in lines[1:]:
    f = ln.split("\t")
    if not f or not f[0]:
        continue
    if f[0] == "rect":
        x, y, w, hh = map(int, f[1:5]); c = tuple(map(int, f[5:8]))
        if w > 0 and hh > 0:
            d.rectangle([x, y, x + w - 1, y + hh - 1], fill=c)
    elif f[0] == "border":
        x, y, w, hh = map(int, f[1:5]); c = tuple(map(int, f[5:8])); t, r, b, l = map(int, f[8:12])
        if w > 0 and hh > 0:
            for bx, by, bw, bh in ((x, y, w, t), (x, y + hh - b, w, b), (x, y, l, hh), (x + w - r, y, r, hh)):
                if bw > 0 and bh > 0:
                    d.rectangle([bx, by, bx + bw - 1, by + bh - 1], fill=c)
    elif f[0] == "text":
        x, y, w, hh = map(int, f[1:5]); c = tuple(map(int, f[5:8])); size = int(f[8])
        d.text((x, y), f[12] if len(f) > 12 else "", fill=c, font=font(size, f[9] == "1", f[10] == "1"))
    elif f[0] == "image":
        x, y, w, hh = map(int, f[1:5])
        if w > 0 and hh > 0:
            d.rectangle([x, y, x + w - 1, y + hh - 1], outline=(200, 0, 200))
img.save(out)
print(f"{out}: {width}x{h} (page height {head[2]}, {head[6]} commands)")
