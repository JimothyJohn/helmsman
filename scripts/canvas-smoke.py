"""Drive the browser build of the canvas in headless Chromium and report
what the native tests cannot see: console errors and panics, and a
screenshot of each step to look at.

usage: python3 -I scripts/canvas-smoke.py [--dist DIR] [--out DIR] [--box]
       make canvas-smoke            (after make canvas-web; --box also
                                     needs cargo build -p helmsman-plc)

Serves DIR (default target/canvas-dist) on a free loopback port and drives
?playground: panes and tabs by hotkey and click, a typed edit, Deploy, the
diagram's "why" click, the Monitor, light mode, and a phone-width Monitor.
With --box it also starts target/debug/helmsman-plc on another free port
serving the same dist, with projects on, HTTPS and OPC UA off and the CIP
ports ephemeral so it cannot collide with a bench box, under a fresh state
directory with an engineer account (alice / alice-engineer-pw), and drives
the box-served page signed out, then signed in. Everything is stopped at
the end.

Needs Playwright's Chromium for python3 (pip install playwright;
python3 -m playwright install chromium). The app draws at 2 CSS px a point
on a doubled viewport at DPR 2 but maps the pointer one to one, so clicks
are given in points of the 1400x860 layout (docs/UI.md). The remembered
layout is seeded into localStorage from a same-origin URL that is not the
app, because eframe saves the app's own layout on beforeunload.

The keys are the ones crates/helmsman-canvas/src/hotkeys.rs binds (the
panes on Alt+Shift+digit, Deploy on Shift+Ctrl/Cmd+Enter). A step whose
key did nothing leaves a screenshot identical to the one before it, which
is reported as a problem: until 2026-11-10 this script pressed Alt+digit
and Ctrl+D, bound to nothing since 1f00a1c, and its files, devices and
deployed shots showed an unchanged screen. The ports are picked free at
start, so two checkouts can run it at once.

Exit status: 1 when a page error or a console error other than the seed
URL's 404 and the signed-out desk read's 401 was seen, else 0.
"""

import argparse
import functools
import hashlib
import http.server
import json
import os
import shutil
import socket
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request

from playwright.sync_api import sync_playwright

KEY = "helmsman-canvas/editor"
W, H = 1400, 860
S = 2
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
USER, PASSWORD = "alice", "alice-engineer-pw"
# Console lines the smoke itself causes.
EXPECTED = ("/__seed", "status of 404", "status of 401")


def layout(left=1, right=1, pane="code", tab="io", scan=0, trend=1, bottom="trend", light=0):
    return f"left={left} right={right} pane={pane} tab={tab} scan={scan} trend={trend} bottom={bottom} light={light}"


# Parts of the 1400x860 layout that hold still while a program runs, in
# points (x, y, width, height): a step that must change one is checked
# against it. The clock, the diagram's values and the trend never hold.
LEFT = (0, 120, 300, 500)
RIGHT_TABS = (1180, 84, 210, 30)
BOTTOM_TABS = (330, 575, 300, 50)
BANNER = (0, 56, 700, 24)
HEADER_RIGHT = (1150, 8, 200, 36)
MOD = "ControlOrMeta"


class Run:
    """One page: its console log and numbered screenshots."""

    def __init__(self, page, out, prefix):
        self.page = page
        self.out = out
        self.prefix = prefix
        self.n = 0
        self.console = []
        self.same = []
        page.on("console", lambda m: self.console.append((m.type, m.text)))
        page.on("pageerror", lambda e: self.console.append(("pageerror", str(e))))

    def shot(self, name):
        self.n += 1
        self.page.screenshot(path=os.path.join(self.out, f"{self.prefix}-{self.n:02d}-{name}.png"))

    def look(self, region):
        x, y, w, h = (v * S for v in region)
        clip = {"x": x, "y": y, "width": w, "height": h}
        return hashlib.sha256(self.page.screenshot(clip=clip)).hexdigest()

    def step(self, name, do, watch, ms=700):
        """`do`, then a screenshot; `watch` (a region) must have changed."""
        before = self.look(watch)
        do()
        self.wait(ms)
        self.shot(name)
        if self.look(watch) == before:
            self.same.append(f"[smoke] {self.prefix}: '{name}' changed nothing on screen")

    def click(self, x, y, button="left"):
        self.page.mouse.click(x, y, button=button)

    def key(self, k):
        self.page.keyboard.press(k)

    def wait(self, ms):
        self.page.wait_for_timeout(ms)

    def problems(self):
        return self.same + [
            f"[{t}] {x[:300]}"
            for (t, x) in self.console
            if t in ("error", "pageerror") and not any(e in x for e in EXPECTED)
        ]


def open_app(ctx, base, query, lay):
    page = ctx.new_page()
    page.goto(base + "/__seed", wait_until="load")
    page.evaluate(f"localStorage.setItem({json.dumps(KEY)}, {json.dumps(lay)})")
    page.goto(base + "/" + query, wait_until="load")
    return page


def playground(ctx, base, out):
    r = Run(open_app(ctx, base, "?playground", layout()), out, "pg")
    r.wait(3000)
    r.shot("editor")
    r.step("files", lambda: r.key("Alt+Shift+Digit0"), LEFT)
    r.step("devices", lambda: r.key("Alt+Shift+Digit4"), LEFT)
    # The Code tab by its tab, not Alt+Shift+1: with Shift a US keyboard
    # off the Mac sends "!", which the page does not take for the 1 key.
    r.step("code", lambda: r.click(183, 98), LEFT)
    # The right panel's tabs: Agent, then I/O (two tabs since 2026-11-08).
    r.click(1290, 98); r.wait(600); r.shot("agent-tab")
    r.click(1232, 98); r.wait(600); r.shot("io-tab")
    for x, name in ((137, "file-menu"), (190, "edit-menu"), (235, "view-menu"), (283, "help-menu")):
        r.click(x, 26); r.wait(500); r.shot(name); r.key("Escape")
    # Type a comment into the program, then deploy it.
    def edit():
        r.click(130, 300); r.wait(200)
        r.page.keyboard.type("(* smoke *)")
    r.step("edited", edit, LEFT, 1200)
    r.step("deployed", lambda: r.key(f"Shift+{MOD}+Enter"), BANNER, 1200)
    r.click(989, 268); r.wait(1000); r.shot("diagram-why")
    r.step("trend-hidden", lambda: r.key("Alt+Shift+Digit2"), BOTTOM_TABS)
    r.key("Alt+Shift+Digit2"); r.wait(400)
    r.step("right-hidden", lambda: r.key("Alt+Shift+Digit3"), RIGHT_TABS)
    r.key("Alt+Shift+Digit3"); r.wait(400)
    r.click(1313, 26); r.wait(1500); r.shot("monitor")
    r.click(1243, 26); r.wait(800); r.shot("editor-again")
    problems = r.problems()
    r.page.close()
    r2 = Run(open_app(ctx, base, "?playground", layout(pane="files", light=1)), out, "pg-light")
    r2.wait(2500); r2.shot("files")
    r2.click(1290, 98); r2.wait(600); r2.shot("agent-tab")
    r2.click(1313, 26); r2.wait(1500); r2.shot("monitor")
    problems += r2.problems()
    r2.page.close()
    return problems


def box(ctx, base, out):
    """The box-served page, signed out, then signed in as the engineer."""
    r = Run(open_app(ctx, base, "", layout()), out, "box")
    r.wait(4000)
    r.shot("signed-out")
    # The sign-in popup is open on a signed-out page: its two fields.
    def sign_in():
        r.click(1230, 100); r.wait(200)
        r.page.keyboard.type(USER)
        r.click(1230, 136); r.wait(200)
        r.page.keyboard.type(PASSWORD)
        r.key("Enter")
    # The box hashes the password: the answer takes a few seconds.
    r.step("signed-in", sign_in, HEADER_RIGHT, 6000)
    r.step("files", lambda: r.key("Alt+Shift+Digit0"), LEFT, 800)
    r.step("devices", lambda: r.key("Alt+Shift+Digit4"), LEFT, 800)
    r.click(1290, 98); r.wait(800); r.shot("agent-tab")
    r.click(1232, 98); r.wait(600); r.shot("io-tab")
    problems = r.problems()
    r.page.close()
    # The Monitor by its URL: the header's switch sits elsewhere once
    # somebody is signed in.
    m = Run(open_app(ctx, base, "?monitor", layout()), out, "box-monitor")
    m.wait(3000); m.shot("signed-in")
    problems += m.problems()
    m.page.close()
    return problems


def phone(browser, base, query, out, prefix):
    ctx = browser.new_context(viewport={"width": 390 * S, "height": 844 * S}, device_scale_factor=S, color_scheme="dark")
    r = Run(open_app(ctx, base, query, layout()), out, prefix)
    r.wait(3000); r.shot("phone")
    problems = r.problems()
    ctx.close()
    return problems


class Quiet(http.server.SimpleHTTPRequestHandler):
    """The dist server without a request log on stdout."""

    def log_message(self, *_args):
        pass


def free_port():
    """A loopback port nothing listens on now."""
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def serve(root):
    """The dist on a free loopback port: the server, whose address has it."""
    handler = functools.partial(Quiet, directory=root)
    srv = socketserver.ThreadingTCPServer(("127.0.0.1", 0), handler)
    srv.daemon_threads = True
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def wait_for(url, secs=30):
    end = time.time() + secs
    while time.time() < end:
        try:
            urllib.request.urlopen(url, timeout=1).read(1)
            return True
        except Exception:
            time.sleep(0.3)
    return False


def start_box(dist, state, port):
    """A box on `port` with an engineer account, in a fresh state dir."""
    plc = os.path.join(ROOT, "target", "debug", "helmsman-plc")
    if not os.path.exists(plc):
        sys.exit("needs target/debug/helmsman-plc: cargo build -p helmsman-plc")
    env = dict(os.environ, STATE_DIRECTORY=state)
    subprocess.run([plc, "account", "add", USER, "engineer"], input=PASSWORD + "\n", text=True, env=env, check=True, capture_output=True)
    proc = subprocess.Popen(
        [plc, "--projects", "--api", f"127.0.0.1:{port}", "--https", "off", "--opcua", "off", "--ui", dist, "--bind", "127.0.0.1", "--encap-port", "0", "--io-port", "0"],
        env=env, cwd=state, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    if not wait_for(f"http://127.0.0.1:{port}/api"):
        proc.kill()
        sys.exit("the smoke box did not answer")
    return proc


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--dist", default=os.path.join(ROOT, "target", "canvas-dist"))
    ap.add_argument("--out", default=os.path.join(ROOT, "target", "canvas-smoke"))
    ap.add_argument("--box", action="store_true", help="also start a loopback box and drive its page")
    a = ap.parse_args()
    if not os.path.exists(os.path.join(a.dist, "index.html")):
        sys.exit(f"no canvas build in {a.dist}: make canvas-web")
    os.makedirs(a.out, exist_ok=True)
    srv = serve(a.dist)
    base = f"http://127.0.0.1:{srv.server_address[1]}"
    problems = []
    proc = None
    state = tempfile.mkdtemp(prefix="helmsman-smoke-")
    try:
        if a.box:
            box_port = free_port()
            proc = start_box(a.dist, state, box_port)
        with sync_playwright() as p:
            browser = p.chromium.launch()
            ctx = browser.new_context(viewport={"width": W * S, "height": H * S}, device_scale_factor=S, color_scheme="dark")
            problems += playground(ctx, base, a.out)
            ctx.close()
            problems += phone(browser, base, "?playground", a.out, "pg")
            if proc:
                bbase = f"http://127.0.0.1:{box_port}"
                ctx = browser.new_context(viewport={"width": W * S, "height": H * S}, device_scale_factor=S, color_scheme="dark")
                problems += box(ctx, bbase, a.out)
                ctx.close()
                problems += phone(browser, bbase, "", a.out, "box")
            browser.close()
    finally:
        srv.shutdown()
        if proc:
            proc.terminate()
            try:
                proc.wait(5)
            except subprocess.TimeoutExpired:
                proc.kill()
        shutil.rmtree(state, ignore_errors=True)
    shots = sorted(f for f in os.listdir(a.out) if f.endswith(".png"))
    print(f"{len(shots)} screenshots in {a.out}")
    if problems:
        print(f"{len(problems)} console problem(s):")
        for x in problems:
            print("  " + x)
        sys.exit(1)
    print("no console errors or panics")


if __name__ == "__main__":
    main()
