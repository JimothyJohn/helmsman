"""The browser half of the parity run (docs/PARITY.md): every tab
docs/FEATURES.md names is opened on the playground and must be drawn,
look unlike the other tabs of its panel, and log no error. A tab only a
box's page has (BOX_ONLY: Help, the simulation leaves it out) is opened on
the page a box serves instead, signed in as an engineer.

usage: python3 -I scripts/parity-tabs.py --dist DIR [--path P]
                                                        (the candidate: DIR served on loopback, the page
                                                        at P: target/canvas-dist, or a checkout's docs/
                                                        with --path /playground, as Pages serves it)
       python3 -I scripts/parity-tabs.py --site URL     (live: URL/playground/)
       [--box BIN [--box-ui DIR]]                       (the box-only tabs on the page BIN serves from DIR:
                                                        a box's canvas build, make canvas-web's
                                                        target/canvas-dist, at its root; default --dist,
                                                        the page at P)
       [--out DIR] (default target/parity-tabs/<dist|site>)

Without --box a box-only tab is listed as `box  tab:KEY` and is no
failure: nothing showed it. With --box the box is started on a free
loopback port (projects on, HTTPS, OPC UA and the CIP ports off or
ephemeral) under a fresh state directory with one engineer account, and
stopped at the end. The playground build (build-playground.sh) is the
simulation wherever it is served, so --box-ui must be a box's build:
served by a box, the playground has no Help tab, and that tab fails.

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
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request

from playwright.sync_api import sync_playwright

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FEATURES = os.path.join(ROOT, "docs", "FEATURES.md")
# The canvas's remembered layout (crates/helmsman-canvas/src/editor.rs) and
# the 1400 x 860 point window drawn at 2 px a point (docs/UI.md).
KEY = "helmsman-canvas/editor"
W, H, S = 1400, 860, 2
# The side pane's tabs, in their first order, and the bottom panel's two
# tabs, the Trend and the Log (`tab:messages`), along the bottom from the
# left pane's edge (2026-11-26: one panel, its tab strip at its left, the
# layout's msgs=1 showing the Log).
PANES = ("claude", "code", "io", "devices", "files", "history")
BOTTOM = ("trend", "messages")
# The left pane's tabs a box's page has and the simulation leaves out: the
# Help tab (?, 2026-11-26). Opened on a box's page with --box.
BOX_ONLY = ("help",)
# Where a tab's content is, in points: the left pane under its tab row, and
# the bottom panel right of its tab strip (the strip at x 323..356, as
# scripts/canvas-smoke.py's MESSAGES), one region for both of its tabs.
PANE_BODY = (0, 90, 300, 225)
BOTTOM_BODY = (360, 575, 1030, 280)
# Console lines the seeding itself causes (the seed URL is not the app).
EXPECTED = ["/__seed", "status of 404", "favicon"]
# On a box's page, before the sign-in: the signed-out reads' 401.
BOX_EXPECTED = ["status of 401"]
# The engineer the box is started with (as scripts/canvas-smoke.py), and the
# page's sign-in form (#helmsman-login, an HTML form).
USER, PASSWORD = "alice", "alice-engineer-pw"
LOGIN = "#helmsman-login"
HEADER_SIGN_IN = (1309, 17)
# Served from loopback (--dist) only: the page asks the deployed demo relay
# for its status, and the relay's CORS allows the public site, not
# 127.0.0.1, so the browser refuses it (and logs the failed load). Live, on
# the site, the same request must succeed, so these stay errors there.
LOOPBACK_ONLY = ["/demo/v1/", "Failed to load resource: net::ERR_FAILED"]


def manifest_tabs(path=FEATURES):
    with open(path, encoding="utf-8") as f:
        return sorted(set(re.findall(r"`tab:([a-z0-9-]+)`", f.read())))


def layout(pane="code", msgs=0, lefts=PANES):
    """The layout string the canvas reads (canvas-smoke.py `layout`)."""
    return (
        f"dock=2 left=1 right=1 lefts={','.join(lefts)} pane={pane}"
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


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def start_box(plc, ui, state):
    """BIN serving `ui` on a free loopback port, with the engineer account in
    a fresh state dir: (process, its base URL). It exits when this script's
    end of its stdin closes (HELMSMAN_EXIT_WITH_STDIN), so it dies with us."""
    env = dict(os.environ, STATE_DIRECTORY=state)
    subprocess.run([plc, "account", "add", USER, "engineer"], input=PASSWORD + "\n", text=True, env=env, check=True, capture_output=True)
    port = free_port()
    log = open(os.path.join(state, "box.log"), "w")
    proc = subprocess.Popen(
        [plc, "--projects", "--api", f"127.0.0.1:{port}", "--https", "off", "--opcua", "off", "--ui", ui,
         "--bind", "127.0.0.1", "--encap-port", "0", "--io-port", "0"],
        env=dict(env, HELMSMAN_EXIT_WITH_STDIN="1"), cwd=state,
        stdin=subprocess.PIPE, stdout=log, stderr=log, text=True,
    )
    base = f"http://127.0.0.1:{port}"
    end = time.time() + 30
    while time.time() < end and proc.poll() is None:
        try:
            urllib.request.urlopen(base + "/api", timeout=1).read(1)
            return proc, base
        except Exception:
            time.sleep(0.3)
    proc.kill()
    log.close()
    with open(os.path.join(state, "box.log"), encoding="utf-8", errors="replace") as f:
        sys.exit(f"the box {plc} did not answer on {base}: {f.read()[-500:]}")


def sign_in(page):
    """Sign in through the page's form as a person would: problems seen."""
    form = page.locator(LOGIN)
    if not form.count():
        page.mouse.click(HEADER_SIGN_IN[0] * S, HEADER_SIGN_IN[1] * S)
    try:
        form.wait_for(state="visible", timeout=5000)
    except Exception:
        return [f"no sign-in form ({LOGIN}) on the box's page"]
    form.locator("input[name=username]").fill(USER)
    pw = form.locator("input[name=password]")
    pw.fill(PASSWORD)
    pw.press("Enter")
    try:
        page.locator(LOGIN).wait_for(state="detached", timeout=15000)
    except Exception:
        return ["the sign-in form stayed after the right password"]
    return []


def open_tab(browser, base, key, out, box=None):
    """Show tab `key`; (problems, digest of its body). `box` is the base URL
    of a box's page, for a box-only tab."""
    if key in PANES:
        lay, region = layout(pane=key), PANE_BODY
    elif key in BOTTOM:
        lay, region = layout(msgs=int(key == "messages")), BOTTOM_BODY
    elif key in BOX_ONLY:
        lay, region = layout(pane=key, lefts=PANES + BOX_ONLY), PANE_BODY
    else:
        return [f"[{key}] docs/FEATURES.md names a tab this script cannot show: add it to PANES, BOTTOM or BOX_ONLY"], None
    on_box = key in BOX_ONLY
    if on_box:
        # A box serves a directory's files but not the directory itself.
        base, query = box, "/index.html?editor"
    else:
        query = "/?playground&editor"
    ctx = browser.new_context(viewport={"width": W * S, "height": H * S}, device_scale_factor=S, color_scheme="dark")
    console = []
    page = ctx.new_page()
    page.on("console", lambda m: console.append((m.type, m.text)))
    page.on("pageerror", lambda e: console.append(("pageerror", str(e))))
    page.goto(base + "/__seed", wait_until="load")
    page.evaluate(f"localStorage.setItem({json.dumps(KEY)}, {json.dumps(lay)})")
    resp = page.goto(base + query, wait_until="load")
    problems = []
    if resp is None or resp.status != 200:
        ctx.close()
        return [f"[{key}] the page answered {resp.status if resp else 'nothing'}"], None
    page.wait_for_timeout(4000)
    expected = list(EXPECTED)
    if on_box:
        page.screenshot(path=os.path.join(out, f"{key}-signed-out.png"))
        problems += [f"[{key}] {x}" for x in sign_in(page)]
        expected += BOX_EXPECTED
        page.wait_for_timeout(2000)
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
        if t in ("error", "pageerror") and not any(e in x for e in expected)
    ]
    ctx.close()
    return problems, digest


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    where = ap.add_mutually_exclusive_group(required=True)
    where.add_argument("--dist", help="a canvas build to serve on loopback (the candidate)")
    where.add_argument("--site", help="the site whose /playground/ is live")
    ap.add_argument("--path", default="", help="where the page is under --dist (/playground for a checkout's docs/)")
    ap.add_argument("--box", help="a helmsman-plc binary: the box-only tabs on the page it serves")
    ap.add_argument("--box-ui", help="the box's canvas build the box serves, the page at its root (default --dist, the page at --path)")
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
    problems, seen, skipped = [], {}, []
    proc, state, box = None, None, None
    if a.box:
        ui, at = (a.box_ui, "") if a.box_ui else (a.dist, a.path)
        if not ui or not os.path.exists(os.path.join(ui + at, "index.html")):
            sys.exit(f"--box needs a canvas build to serve (--box-ui, else --dist): none at {ui}{at}: make canvas-web")
        state = tempfile.mkdtemp(prefix="helmsman-parity-tabs-")
    try:
        if state:
            # The box runs in its state dir: the paths it is given are absolute.
            proc, box_root = start_box(os.path.abspath(a.box), os.path.abspath(ui), state)
            box = box_root + at.rstrip("/")
        with sync_playwright() as p:
            browser = p.chromium.launch()
            for key in tabs:
                if key in BOX_ONLY and box is None:
                    skipped.append(key)
                    continue
                pr, digest = open_tab(browser, base, key, out, box)
                problems += pr
                if digest is not None:
                    seen.setdefault(digest, []).append(key)
            browser.close()
    finally:
        if srv:
            srv.shutdown()
        if proc:
            proc.stdin.close()
            try:
                proc.wait(5)
            except subprocess.TimeoutExpired:
                proc.kill()
        if state:
            shutil.rmtree(state, ignore_errors=True)
    for keys in seen.values():
        if len(keys) > 1:
            problems.append(f"tabs {', '.join(keys)} show the same thing: one of them did not open")
    print(f"{len(tabs)} tabs from {os.path.relpath(FEATURES, ROOT)} on {base}{f' and {box}' if box else ''}; screenshots in {out}")
    for t in tabs:
        if t in skipped:
            print(f"box  tab:{t} (only a box's page has it: give --box to show it)")
            continue
        bad = [x for x in problems if x.startswith(f"[{t}]") or f" {t}," in x or x.endswith(f" {t} show")]
        print(f"{'FAIL' if bad else 'ok  '} tab:{t}")
    if problems:
        print("\n".join(problems))
        sys.exit(1)


if __name__ == "__main__":
    main()
