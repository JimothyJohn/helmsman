"""The browser half of the parity run (docs/PARITY.md): every tab
docs/FEATURES.md names is opened on the playground and must be drawn,
look unlike the other tabs of its panel, and log no error.

usage: python3 -I scripts/parity-tabs.py --dist DIR [--path P]
                                                        (the candidate: DIR served on loopback, the page
                                                        at P: target/canvas-dist, or a checkout's docs/
                                                        with --path /playground, as Pages serves it)
       python3 -I scripts/parity-tabs.py --site URL     (live: URL/playground/)
       [--out DIR] (default target/parity-tabs/<dist|site>)

The tabs come from the manifest (`tab:KEY`), so a tab added to the canvas
is opened here as soon as its row is (helmsman-canvas features_tests.rs
holds the rows to the tabs). Each tab is shown by the remembered layout,
seeded before the page loads, the way scripts/canvas-smoke.py does it:
no hotkey or click position to drift. A manifest tab this script does not
know how to show is a problem, named.

Reads only; it sends nothing to the Agent. Needs Playwright's Chromium for
python3 (as scripts/canvas-smoke.py). Exit 1 when a tab failed.
"""
import argparse
import functools
import hashlib
import http.server
import json
import os
import re
import sys
import threading

from playwright.sync_api import sync_playwright

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FEATURES = os.path.join(ROOT, "docs", "FEATURES.md")
# The canvas's remembered layout (crates/helmsman-canvas/src/editor.rs) and
# the 1400 x 860 point window drawn at 2 px a point (docs/UI.md).
KEY = "helmsman-canvas/editor"
W, H, S = 1400, 860, 2
# The side pane's tabs, in their first order, and the panels that are not
# tabs: the trend along the whole bottom and the Messages in the left pane
# under its tabs (2026-11-23).
PANES = ("claude", "code", "io", "devices", "files", "history")
BOTTOM = ("trend", "messages")
# Where a tab's content is, in points: the left pane under its tab row, the
# trend under the diagram, the Messages above it.
PANE_BODY = (0, 90, 300, 225)
BOTTOM_BODY = {"trend": (0, 545, 1400, 315), "messages": (0, 330, 300, 200)}
# Console lines the seeding itself causes (the seed URL is not the app).
EXPECTED = ["/__seed", "status of 404", "favicon"]
# Served from loopback (--dist) only: the page asks the deployed demo relay
# for its status, and the relay's CORS allows the public site, not
# 127.0.0.1, so the browser refuses it (and logs the failed load). Live, on
# the site, the same request must succeed, so these stay errors there.
LOOPBACK_ONLY = ["/demo/v1/", "Failed to load resource: net::ERR_FAILED"]


def manifest_tabs(path=FEATURES):
    with open(path, encoding="utf-8") as f:
        return sorted(set(re.findall(r"`tab:([a-z0-9-]+)`", f.read())))


def layout(pane="code", msgs=1):
    """The layout string the canvas reads (canvas-smoke.py `layout`)."""
    return (
        f"dock=2 left=1 right=1 lefts={','.join(PANES)} pane={pane}"
        f" scan=0 trend=1 msgs={msgs} light=0"
    )


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *a):
        pass


def serve(dist):
    handler = functools.partial(Quiet, directory=dist)
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def shot(page, region, path):
    x, y, w, h = (v * S for v in region)
    png = page.screenshot(path=path, clip={"x": x, "y": y, "width": w, "height": h})
    return hashlib.sha256(png).hexdigest(), len(png)


def open_tab(browser, base, key, out):
    """Show tab `key`; (problems, digest of its body)."""
    if key in PANES:
        lay, region = layout(pane=key), PANE_BODY
    elif key in BOTTOM:
        lay, region = layout(), BOTTOM_BODY[key]
    else:
        return [f"[{key}] docs/FEATURES.md names a tab this script cannot show: add it to PANES or BOTTOM"], None
    ctx = browser.new_context(viewport={"width": W * S, "height": H * S}, device_scale_factor=S, color_scheme="dark")
    console = []
    page = ctx.new_page()
    page.on("console", lambda m: console.append((m.type, m.text)))
    page.on("pageerror", lambda e: console.append(("pageerror", str(e))))
    page.goto(base + "/__seed", wait_until="load")
    page.evaluate(f"localStorage.setItem({json.dumps(KEY)}, {json.dumps(lay)})")
    resp = page.goto(base + "/?playground&editor", wait_until="load")
    problems = []
    if resp is None or resp.status != 200:
        ctx.close()
        return [f"[{key}] the page answered {resp.status if resp else 'nothing'}"], None
    page.wait_for_timeout(4000)
    page.screenshot(path=os.path.join(out, f"{key}-page.png"))
    # The loader's own error screen is HTML, so it can be read.
    body = page.inner_text("body")
    if "could not start" in body:
        ctx.close()
        return [f"[{key}] the page did not start: {' '.join(body.split())[:300]}"], None
    digest, size = shot(page, region, os.path.join(out, f"{key}.png"))
    # A flat region compresses to almost nothing: the tab drew no content.
    if size < 4000:
        problems.append(f"[{key}] the tab's body is one flat colour ({size} bytes)")
    problems += [
        f"[{key}] [{t}] {x[:300]}"
        for (t, x) in console
        if t in ("error", "pageerror") and not any(e in x for e in EXPECTED)
    ]
    ctx.close()
    return problems, digest


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    where = ap.add_mutually_exclusive_group(required=True)
    where.add_argument("--dist", help="a canvas build to serve on loopback (the candidate)")
    where.add_argument("--site", help="the site whose /playground/ is live")
    ap.add_argument("--path", default="", help="where the page is under --dist (/playground for a checkout's docs/)")
    ap.add_argument("--out")
    a = ap.parse_args()
    out = a.out or os.path.join(ROOT, "target", "parity-tabs", "dist" if a.dist else "site")
    os.makedirs(out, exist_ok=True)
    srv = None
    if a.dist:
        if not os.path.exists(os.path.join(a.dist, "index.html")):
            sys.exit(f"no canvas build in {a.dist}: make canvas-web")
        srv = serve(a.dist)
        EXPECTED.extend(LOOPBACK_ONLY)
        base = f"http://127.0.0.1:{srv.server_address[1]}" + a.path.rstrip("/")
    else:
        base = a.site.rstrip("/") + "/playground"
    tabs = manifest_tabs()
    problems, seen = [], {}
    with sync_playwright() as p:
        browser = p.chromium.launch()
        for key in tabs:
            pr, digest = open_tab(browser, base, key, out)
            problems += pr
            if digest is not None:
                seen.setdefault(digest, []).append(key)
        browser.close()
    if srv:
        srv.shutdown()
    for keys in seen.values():
        if len(keys) > 1:
            problems.append(f"tabs {', '.join(keys)} show the same thing: one of them did not open")
    print(f"{len(tabs)} tabs from {os.path.relpath(FEATURES, ROOT)} on {base}; screenshots in {out}")
    for t in tabs:
        bad = [x for x in problems if x.startswith(f"[{t}]") or f" {t}," in x or x.endswith(f" {t} show")]
        print(f"{'FAIL' if bad else 'ok  '} tab:{t}")
    if problems:
        print("\n".join(problems))
        sys.exit(1)


if __name__ == "__main__":
    main()
