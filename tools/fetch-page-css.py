#!/usr/bin/env python3
"""fetch-page-css.py URL OUT_DIR

Downloads URL and the style sheets it links (and their @imports, one level) into OUT_DIR as
page.html and css-N.css, in the order the browser engine applies them. Prints the list of CSS files.
For `tools/render-page.py PAGE --css FILES...`."""
import os, re, subprocess, sys, urllib.parse

url, out = sys.argv[1], sys.argv[2]
os.makedirs(out, exist_ok=True)

def get(u):
    r = subprocess.run(["curl", "-sL", "-m", "30", "-A", "Mozilla/5.0", "--compressed", u], capture_output=True)
    return r.stdout

html = get(url)
open(os.path.join(out, "page.html"), "wb").write(html)
links = subprocess.run(
    ["cargo", "run", "-q", "--release", "-p", "kitsune_core", "--example", "render_page", "--",
     os.path.join(out, "page.html"), "960", "100", "--links"],
    capture_output=True, text=True).stdout.split()
files = []
seen = set()
def add(href, base, depth=0):
    full = urllib.parse.urljoin(base, href)
    if full in seen or len(files) >= 12:
        return
    seen.add(full)
    css = get(full)
    name = os.path.join(out, f"css-{len(files)}.css")
    open(name, "wb").write(css)
    files.append(name)
    if depth < 1:
        for m in re.finditer(rb"@import\s+(?:url\(\s*)?['\"]?([^'\")\s;]+)", css):
            add(m.group(1).decode(), full, depth + 1)
for l in links:
    add(l, url)
print(" ".join(files))
