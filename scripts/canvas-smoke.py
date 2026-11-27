"""Drive the browser build of the canvas in headless Chromium and report
what the native tests cannot see: console errors and panics, and a
screenshot of each step to look at.

usage: python3 -I scripts/canvas-smoke.py [--dist DIR] [--out DIR] [--box]
                                          [--browser chromium|firefox|webkit|all] [--no-gpu]
       make canvas-smoke            (after make canvas-web; --box also
                                     needs cargo build -p helmsman-plc)

Serves DIR (default target/canvas-dist) on a free loopback port and drives
?playground: a first visit (nothing kept, the old front door's key kept,
an old ?playground&front link) opening on the editor, panes and tabs by
hotkey and click, the view keys (L the Log,
T the Trend, Z fit, Space run and stop), the side pane's ribbon, a typed
edit, Deploy, the diagram's "why" click, Scan for Device and a found device
added by name, the Monitor, light mode, and a phone-width Monitor.
With --box it also starts target/debug/helmsman-plc on another free port
serving the same dist, with projects on, HTTPS and OPC UA off and the CIP
ports ephemeral so it cannot collide with a bench box, under a fresh state
directory with an engineer account (alice / alice-engineer-pw), and drives
the box-served page signed out, then signed in through the sign-in form
(#helmsman-login, a real HTML form the browser's password manager can
save: its fields, Esc, Cancel, letters typed in it, the Proposals screen's
Sign in…, a wrong password, and the password nowhere in storage, a URL or
the console). Everything is stopped at the end.

--browser picks the engine (default chromium; all runs the three, WebKit
standing in for Safari). With all, each engine runs twice: as the engine
picks its drawing, and with software drawing forced (SwiftShader flags for
Chromium, software WebRender and WebGL prefs for Firefox; WebKit headless
has no switch, so its two runs differ only in name). --no-gpu forces
software drawing for a single --browser. Screenshots of a run go to
OUT/<engine>[-nogpu]/ (OUT itself for plain Chromium), and the run logs
the WebGL2 renderer the engine gave the page. A last scenario makes
getContext('webgl2') return null and asserts the page's plain notice and
no console error (the app must not start, let alone panic).

Needs Playwright's browsers for python3 (pip install playwright;
python3 -m playwright install chromium firefox webkit). The app draws at 2 CSS px a point
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

from playwright.sync_api import TimeoutError as PlaywrightTimeout
from playwright.sync_api import sync_playwright

PLC = None  # set in main: target/debug/helmsman-plc

KEY = "helmsman-canvas/editor"
W, H = 1400, 860
S = 2
# CSS pixels per point of the layout, per engine, set in main. Chromium and
# Firefox with an emulated device_scale_factor give eframe a canvas buffer of
# CSS size while it assumes the DPR, so the 1400 x 860 point layout spans
# 2800 x 1720 CSS px (2 per point). WebKit behaves as a real Retina screen:
# 1 CSS px per point on a 1400 x 860 viewport. Measured 2026-11-22 (canvas
# width attribute 1400 vs 2800 at a 1400 CSS px viewport and DPR 2).
K = 2
# --headed: show the browser (under xvfb-run on a runner). Headless Firefox on
# Linux may have no GL context at all; a headed one under a virtual display does.
HEADED = False
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
USER, PASSWORD = "alice", "alice-engineer-pw"
# Console lines the smoke itself causes.
EXPECTED = ("/__seed", "status of 404", "status of 401", "status of 409", "status of 422")


# Every tab in the left pane, in its first order (2026-11-13: one pane, the
# Agent first; a tab can be docked in a right pane). Help (?) is last, on a
# box only: the simulation leaves it out (2026-11-26).
PANES = ("claude", "code", "io", "devices", "files", "help")


def layout(left=1, pane="code", rights="", scan=0, trend=1, msgs=1, light=0):
    docked = [p for p in rights.split(",") if p]
    lefts = ",".join(p for p in PANES if p not in docked)
    right = f" rights={rights} side={docked[0]}" if docked else ""
    return (
        f"dock=2 left={left} right=1 lefts={lefts} pane={pane}{right}"
        f" scan={scan} trend={trend} msgs={msgs} light={light}"
    )


# Parts of the 1400x860 layout that hold still while a program runs, in
# points (x, y, width, height): a step that must change one is checked
# against it. The clock, the diagram's values and the trend never hold.
# Re-measured with the native harness at 1400 x 860 on 2026-11-27, when
# the chrome started to float 8 pt in from the edges under a taller header
# (docs/UI.md "Look and feel").
LEFT = (0, 100, 300, 200)
# Where a right pane's tabs show once one is docked there.
RIGHT_TABS = (1180, 96, 210, 42)
# The trend is the bottom strip from the left pane's edge since 2026-11-24
# (the left pane has the page's whole height).
BOTTOM_TABS = (323, 585, 1070, 270)
# The Log (the Messages, renamed 2026-11-26): a tab of the bottom panel, its
# list to the right of the panel's tab strip (the strip at x 331..364 with
# the left pane's frame ending near 298 pt); layout()'s msgs=1 shows it. What a deploy says
# is there, not in a line under the header.
MESSAGES = (364, 589, 1020, 250)
# The diagram.
DIAGRAM = (330, 130, 1000, 440)
# View › Pane layouts…'s popup, in the middle of the page.
LAYOUTS_POPUP = (540, 290, 320, 340)
# The Devices tab's Scan for Device under the (empty) list, the simulated
# E1213 it finds under it, and the small form's Add (2026-11-26). Measured
# with the native harness at 1400 x 860 points.
SCAN_FOR_DEVICE = (93, 235)
FOUND_DEVICE = (151, 269)
QUICK_ADD = (36, 499)
# The left pane's hide button on its ribbon, at the pane's vertical middle,
# and shut, the strip's expand glyph at the same height (2026-11-26).
RIBBON_LEFT = (280, 470)
STRIP_LEFT = (28, 470)
# The simulation's run state in the diagram's header ("running", its clock
# beside it).
RUN_STATE = (900, 92, 190, 32)
# The tabs are glyphs (2026-11-11), all seven in the left pane's row since
# 2026-11-13 (29.4 pt each at 320 pt).
TAB_AGENT = (32, 118)
TAB_CODE = (74, 118)
TAB_IO = (114, 118)
# One tab docked on the right: the first of its row.
TAB_RIGHT_FIRST = (1218, 118)
# The fold arrow of the Code tab's second line, in the gutter.
FOLD_LINE_2 = (24, 245)
# The one menu (the three-line glyph, 2026-11-23), its four submenus, the
# View submenu's "Pane layouts…" item and, on a box's page, its Proposals
# item (the box's View menu has two more rows above Pane layouts:
# Proposals and Simulate in this browser). The submenus open to the right
# of the menu. Measured with the native harness, 1400 x 860 points.
MENU = (190, 40)
MENU_FILE = (215, 78)
MENU_EDIT = (215, 108)
MENU_VIEW = (215, 138)
MENU_HELP = (215, 168)
VIEW_PANE_LAYOUTS = (404, 545)
# The box's View menu lists the Help tab above Pane layouts (2026-11-26).
# Re-measured with the native harness on a signed-in box page on
# 2026-11-27 (`view layouts` at 269..539, 646..676): (396, 615) had become
# View › Help.
VIEW_PANE_LAYOUTS_BOX = (404, 661)
VIEW_PROPOSALS_BOX = (396, 198)
LAYOUT_DEFAULT = (669, 324)
LAYOUT_PROGRAMMING = (669, 363)
# The header's right end (Editor | Monitor, Sign in or the user, Help),
# its items centred at y 40 under the floating chrome (2026-11-27).
HEADER_RIGHT = (1130, 20, 260, 40)
# The header's Editor | Monitor toggle, each side.
TOGGLE_EDITOR = (1255, 40)
TOGGLE_MONITOR = (1312, 40)
MOD = "ControlOrMeta"
# The header's Sign in, and the Proposals screen's "Sign in…" on a box
# signed out, in points. Re-measured on 2026-11-27 under the floating
# chrome (the header is 85 pt tall, its items centred at y 40): the
# header's from the native harness (`sign in` at 1263..1341, 25..55), the
# Proposals screen's from the smoke's screenshot.
HEADER_SIGN_IN = (1302, 40)
PROPOSALS_SIGN_IN = (54, 181)
# The Monitor's bottom panel: its Trend tab, the first of the strip at the
# page's left (native harness, 1400 x 860: `bottom trend` 24..57, 591..624).
MONITOR_TREND_TAB = (40, 607)
# The Deploy sheet ("What changes on the machine") on a box: its Cancel.
# The sheet's Deploy and Cancel are pinned to its bottom edge, which sits a
# fixed 58 pt above the window's (what it shows scrolls above them), so
# the button is found from the window's height, whatever the sheet holds:
# measured with the native harness at 1400 x 860 and 1400 x 1180 for two
# programs on 2026-11-27 (`deploy sheet cancel` at 377..457, H-88..H-58).
# The program has no I/O, so the box needs no map.
SHEET_CANCEL = (417, H - 73)
SHEET_PROGRAM = "PROGRAM tally\nVAR\n  n : INT;\nEND_VAR\nn := n + 1;\nEND_PROGRAM\n"
# The box sign-in in the browser build is a real HTML form, so the
# browser's password manager can fill and save it (Nick, 2026-11-26).
LOGIN = "#helmsman-login"
WRONG_PASSWORD = "not-alices-pw-4821"


class Run:
    """One page: its console log and numbered screenshots."""

    def __init__(self, page, out, prefix, expect=()):
        self.page = page
        self.out = out
        self.prefix = prefix
        # Console lines this page's own steps cause, beside EXPECTED.
        self.expect = tuple(expect)
        self.n = 0
        self.console = []
        self.same = []
        self.urls = []
        page.on("console", lambda m: self.console.append((m.type, m.text)))
        page.on("request", lambda q: self.urls.append(q.url))
        page.on("pageerror", lambda e: self.console.append(("pageerror", str(e))))

    def shot(self, name):
        self.n += 1
        self.page.screenshot(path=os.path.join(self.out, f"{self.prefix}-{self.n:02d}-{name}.png"))

    def look(self, region):
        x, y, w, h = (v * K for v in region)
        clip = {"x": x, "y": y, "width": w, "height": h}
        return hashlib.sha256(self.page.screenshot(clip=clip)).hexdigest()

    def step(self, name, do, watch, ms=700, stays=False):
        """`do`, then a screenshot; `watch` (a region) must have changed,
        or with `stays` must not have: a build without the free chat shows
        "Describing a change is not available in this build." in the tab,
        with no design button or window to open, so a click or typing there
        leaves the pane as it was."""
        before = self.look(watch)
        do()
        self.wait(ms)
        self.shot(name)
        changed = self.look(watch) != before
        if changed == stays:
            what = "changed" if changed else "changed nothing"
            self.same.append(f"[smoke] {self.prefix}: '{name}' {what} on screen")

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
            if t in ("error", "pageerror") and not any(e in x for e in EXPECTED + self.expect)
        ]


# "Design a machine..." in the Agent tab of a new project (about where the
# button sits), in points.
AGENT_DESIGN = (78, 308)

def open_app(ctx, base, query, lay):
    page = ctx.new_page()
    page.goto(base + "/__seed", wait_until="load")
    if lay is None:
        page.evaluate(f"localStorage.removeItem({json.dumps(KEY)})")
    else:
        page.evaluate(f"localStorage.setItem({json.dumps(KEY)}, {json.dumps(lay)})")
    page.goto(base + "/" + query, wait_until="load")
    return page


def playground(ctx, base, out):
    r = Run(open_app(ctx, base, "?playground&editor", layout()), out, "pg")
    r.wait(3000)
    r.shot("editor")
    r.step("files", lambda: r.key("Alt+Shift+Digit0"), LEFT)
    r.step("devices", lambda: r.key("Alt+Shift+Digit4"), LEFT)
    # The Code tab by its tab, not Alt+Shift+1: with Shift a US keyboard
    # off the Mac sends "!", which the page does not take for the 1 key.
    r.step("code", lambda: r.click(*TAB_CODE), LEFT)
    # Ctrl F with the keyboard off the canvas (as after a click on the
    # browser's own chrome) opens the program's find row over the code,
    # not the browser's find (loading/loading.js); Esc shuts it again.
    def find_off_canvas():
        r.page.evaluate("document.activeElement && document.activeElement.blur()")
        r.key(f"{MOD}+f")
    r.step("find-off-canvas", find_off_canvas, LEFT)
    r.step("find-closed", lambda: r.key("Escape"), LEFT, 400)
    # Fold the section on line 2 (traffic.st's VAR_INPUT) by its gutter arrow,
    # then open it.
    r.step("folded", lambda: r.click(*FOLD_LINE_2), LEFT)
    r.click(*FOLD_LINE_2); r.wait(300)
    r.click(*TAB_AGENT); r.wait(600); r.shot("agent-tab")
    # This build has no free chat: the tab says so (UNAVAILABLE_TEXT) and a
    # click or typing on it changes nothing. (With a design entry:
    # "Design a machine..." opens its window; text typed into it shows.)
    # The pointer off the tab first: the tab's hover eases out (docs/UI.md
    # "Look and feel", finish 10), which would read as the click's change.
    r.page.mouse.move(*AGENT_DESIGN); r.wait(400)
    r.step("design-window", lambda: r.click(*AGENT_DESIGN), LEFT, stays=True)
    # (Typing here is no longer a no-op: with no field focused a letter is a
    # view's key, so the page switches views; the tests cover the keys.)
    r.key("Escape"); r.wait(400)
    r.click(*TAB_IO); r.wait(600); r.shot("io-tab")
    r.click(*TAB_CODE); r.wait(400)
    for sub, name in ((MENU_FILE, "file-menu"), (MENU_EDIT, "edit-menu"), (MENU_VIEW, "view-menu"), (MENU_HELP, "help-menu")):
        r.click(*MENU); r.wait(300); r.click(*sub); r.wait(500); r.shot(name); r.key("Escape"); r.key("Escape")
    # Help > Show hotkeys writes each shortcut over its button; one more
    # click puts the page back as it was.
    # (One row higher since Help › Front page went, 2026-11-27; measured
    # with the native harness under the floating chrome the same day.)
    HELP_HOTKEYS = (354, 303)
    def hotkeys_menu():
        r.click(*MENU); r.wait(300); r.click(*MENU_HELP); r.wait(500); r.click(*HELP_HOTKEYS)
    r.step("hotkeys-on", hotkeys_menu, LEFT, 700)
    r.step("hotkeys-off", hotkeys_menu, LEFT, 700)
    # Type a comment into the program, then deploy it.
    def edit():
        # A comment typed where the caret lands (egui's End chord moves
        # within the row): any comment compiles. A click aimed at a line
        # depends on how the lines wrap at the pane's width.
        r.click(130, 250); r.wait(200)
        r.key(f"{MOD}+End"); r.wait(100)
        r.page.keyboard.type("\n(* smoke *)")
    r.step("edited", edit, LEFT, 1200)
    r.step("deployed", lambda: r.key(f"Shift+{MOD}+Enter"), MESSAGES, 1200)
    r.click(989, 268); r.wait(1000); r.shot("diagram-why")
    # A right click on the GREEN row's AND block opens its menu (Chat about
    # this); Escape shuts it. The diagram never changes the code.
    r.step("diagram-menu", lambda: r.click(503, 381, "right"), DIAGRAM)
    r.step("diagram-menu-closed", lambda: r.key("Escape"), DIAGRAM, 400)
    # A drag on empty space draws a selection box (the blocks it touches
    # are outlined blue); Escape deselects.
    def band():
        m = r.page.mouse
        m.move(345, 495); m.down()
        m.move(800, 200, steps=12); m.up()
    r.step("diagram-selected", band, DIAGRAM, 800)
    r.step("diagram-deselected", lambda: r.key("Escape"), DIAGRAM, 500)
    r.step("trend-hidden", lambda: r.key("Alt+Shift+Digit2"), BOTTOM_TABS)
    r.key("Alt+Shift+Digit2"); r.wait(400)
    # The view keys (2026-11-26): T the Trend, L the Log, each shutting the
    # panel when its tab shows; Z fits the diagram; Space stops and runs
    # the simulation (on release; a box would ask first).
    r.page.mouse.move(700, 300); r.wait(200)
    r.step("key-t-trend", lambda: r.key("t"), BOTTOM_TABS)
    r.step("key-l-log", lambda: r.key("l"), BOTTOM_TABS)
    r.step("key-l-shut", lambda: r.key("l"), BOTTOM_TABS)
    r.key("l"); r.wait(400)
    def zoomed_in():
        r.page.mouse.move(700, 300)
        for _ in range(6):
            r.page.mouse.wheel(0, -240); r.wait(60)
    r.step("zoomed-in", zoomed_in, DIAGRAM, 600)
    r.step("key-z-fit", lambda: r.key("z"), DIAGRAM, 900)
    r.step("space-stopped", lambda: r.key("Space"), RUN_STATE, 600)
    r.step("space-running", lambda: r.key("Space"), RUN_STATE, 600)
    # The left pane's ribbon: its button hides the pane, the strip's glyph
    # at the same height brings it back.
    r.step("ribbon-shut", lambda: r.click(*RIBBON_LEFT), LEFT, 700)
    r.step("ribbon-open", lambda: r.click(*STRIP_LEFT), LEFT, 700)
    # Alt+Shift+5 docks the Code tab on the right, the I/O shown on the
    # left; Alt+Shift+3 hides and shows that pane; the tab dragged back
    # onto the left pane returns.
    r.step("docked-right", lambda: r.key("Alt+Shift+Digit5"), RIGHT_TABS)
    r.step("right-hidden", lambda: r.key("Alt+Shift+Digit3"), RIGHT_TABS)
    r.key("Alt+Shift+Digit3"); r.wait(400)
    def drag_back():
        m = r.page.mouse
        # A frame between each phase: egui starts a drag from the press it
        # saw on a hovered tab, and WebKit delivers the events faster than
        # the page draws.
        m.move(*TAB_RIGHT_FIRST); r.wait(150); m.down(); r.wait(150)
        m.move(160, 300, steps=12); r.wait(150); m.up()
    r.step("docked-back", drag_back, RIGHT_TABS)
    # View › Pane layouts… (2026-11-15): the popup, then the ready-made
    # Programming layout (the code docked on the right), then Default.
    def layouts():
        r.click(*MENU); r.wait(500)
        r.click(*MENU_VIEW); r.wait(1500)
        r.click(*VIEW_PANE_LAYOUTS)
    r.step("pane-layouts", layouts, LAYOUTS_POPUP, 1500)
    r.step("layout-programming", lambda: r.click(*LAYOUT_PROGRAMMING), RIGHT_TABS, 1000)
    layouts(); r.wait(1500)
    r.step("layout-default", lambda: r.click(*LAYOUT_DEFAULT), RIGHT_TABS, 1000)
    r.click(*TOGGLE_MONITOR); r.wait(1500); r.shot("monitor")
    r.click(*TOGGLE_EDITOR); r.wait(800); r.shot("editor-again")
    # A program the checker warns of (RETAIN where it is not kept): marked
    # in the editor and listed under it, and it still compiles. The Code
    # tab first: the Default layout above leaves the Agent's tab showing,
    # and the typing went nowhere (seen in pg-42-warned.png, 2026-11-27).
    r.click(*TAB_CODE); r.wait(400)
    r.click(130, 300); r.key("Control+a")
    r.page.keyboard.type(
        "FUNCTION_BLOCK Latch\nVAR_INPUT set : BOOL; END_VAR\nVAR_OUTPUT q : BOOL; END_VAR\n"
        "VAR RETAIN kept : INT; END_VAR\nq := q OR set;\nEND_FUNCTION_BLOCK\n"
        "PROGRAM main\nVAR_INPUT go : BOOL; END_VAR\nVAR_OUTPUT lamp : BOOL; END_VAR\n"
        "VAR l : Latch; END_VAR\nl(set := go);\nlamp := l.q;\nEND_PROGRAM\n"
    )
    r.wait(1500); r.shot("warned")
    # A slip that does not compile: once the typing stops, the Agent says
    # what went wrong under the top bar, with Fix it (crate::watch).
    r.key("Control+a")
    r.page.keyboard.type(
        "PROGRAM main\nVAR_INPUT go : BOOL; END_VAR\nVAR_OUTPUT lamp : BOOL; END_VAR\n"
        "lmap := go;\nEND_PROGRAM\n"
    )
    r.wait(3500); r.shot("agent-noticed")
    # Last on this page: Scan for Device lists the simulated E1213, a click
    # asks only its name, Add adds it (2026-11-26; Wire it is off until its
    # walkthrough works). After the program's own steps, because adding
    # the device copies its profile into the project and the Code tab opens
    # on that file: the fold, the typed comment and the two programs above
    # would land in it.
    r.step("devices-again", lambda: r.key("Alt+Shift+Digit4"), LEFT)
    r.step("scan-for-device", lambda: r.click(*SCAN_FOR_DEVICE), LEFT, 1200)
    r.step("found-device-name", lambda: r.click(*FOUND_DEVICE), LEFT, 900)
    r.step("sim-device-added", lambda: r.click(*QUICK_ADD), LEFT, 1500)
    r.shot("sim-device-rows")
    problems = r.problems()
    r.page.close()
    # No layout kept: the page opens on the Agent, every tab on the left.
    r0 = Run(open_app(ctx, base, "?playground&editor", None), out, "pg-default")
    r0.wait(2500); r0.shot("agent-first")
    problems += r0.problems()
    r0.page.close()
    r2 = Run(open_app(ctx, base, "?playground&editor", layout(pane="files", light=1)), out, "pg-light")
    r2.wait(2500); r2.shot("files")
    r2.click(*TAB_AGENT); r2.wait(600); r2.shot("agent-tab")
    r2.click(*TOGGLE_MONITOR); r2.wait(1500); r2.shot("monitor")
    problems += r2.problems()
    r2.page.close()
    return problems


# What a browser kept from the front door (gone 2026-11-27): the key it
# set once the visitor went on to the editor.
OLD_FRONT_KEY = "helmsman-canvas/front"


def first_visit(browser, base, out):
    """A first visit opens on the editor, on the default project, with no
    front door (Nick, 2026-11-27): each case in a fresh browser context
    (nothing kept, or only what the case puts there) must show the left
    pane exactly as `?playground&editor` with nothing kept shows it. The
    old front door covered the whole page, so it cannot pass. The cases:
    the public page's `?playground`, the same with the old front door's key
    in storage, and an old `?playground&front` link."""

    def visit(prefix, query, kept):
        ctx = browser.new_context(viewport={"width": W * K, "height": H * K}, device_scale_factor=S, color_scheme="dark")
        page = ctx.new_page()
        r = Run(page, out, prefix)
        page.goto(base + "/__seed", wait_until="load")
        page.evaluate("localStorage.clear()")
        for k, v in kept.items():
            page.evaluate(f"localStorage.setItem({json.dumps(k)}, {json.dumps(v)})")
        page.goto(base + "/" + query, wait_until="load")
        r.wait(3500)
        r.shot("opened")
        left = r.look(LEFT)
        # A while later: nothing has come up in front of the editor.
        r.wait(6000)
        r.shot("later")
        later = r.look(LEFT)
        problems = r.problems()
        ctx.close()
        return left, later, problems

    want, _, problems = visit("pg-first-reference", "?playground&editor", {})
    for prefix, query, kept in (
        ("pg-first-visit", "?playground", {}),
        ("pg-first-old-key", "?playground", {OLD_FRONT_KEY: "seen"}),
        ("pg-first-old-link", "?playground&front", {}),
    ):
        left, later, found = visit(prefix, query, kept)
        problems += found
        if left != want:
            problems.append(f"[smoke] {prefix}: {query} with {kept or 'nothing'} kept does not open on the editor as ?playground&editor does (see {prefix}-01-opened.png)")
        elif later != want:
            problems.append(f"[smoke] {prefix}: {query} left the editor within 10 s (see {prefix}-02-later.png)")
    return problems


def login_form(r, open_with=HEADER_SIGN_IN):
    """The sign-in form, opened with a click at `open_with` when it is not
    up; None when no click brings it."""
    form = r.page.locator(LOGIN)
    if not form.count():
        r.click(*open_with)
        try:
            form.wait_for(state="visible", timeout=3000)
        except PlaywrightTimeout:
            return None
    return form


def said(r):
    """What the sign-in form's problem line says, or None with no form."""
    line = r.page.locator(LOGIN + " p.problem")
    return line.inner_text() if line.count() else None


def dom_sign_in(r, user, password):
    """Sign in through the header's form, as a person (or the browser's
    password manager) fills it: the problems seen."""
    form = login_form(r)
    if form is None:
        return [f"[smoke] {r.prefix}: no sign-in form ({LOGIN}) to sign in with"]
    form.locator("input[name=username]").fill(user)
    pw = form.locator("input[name=password]")
    pw.fill(password)
    pw.press("Enter")
    # The box hashes the password: the answer takes a few seconds. On
    # success the form leaves the page (the cue a browser offers to save on).
    try:
        r.page.locator(LOGIN).wait_for(state="detached", timeout=15000)
    except PlaywrightTimeout:
        return [f"[smoke] {r.prefix}: the sign-in form stayed after the right password (it says {said(r)!r})"]
    return []


# Everything a page keeps or shows outside its canvas: storage and its URL.
KEPT = "() => JSON.stringify([Object.entries(localStorage), Object.entries(sessionStorage), location.href, document.title])"


def leaks(r, *secrets):
    """A password in the page's storage, its URL, a request URL or the console."""
    kept = r.page.evaluate(KEPT)
    found = []
    for i, x in enumerate(secrets):
        where = [
            name
            for name, hit in (
                ("localStorage, sessionStorage or the URL", x in kept),
                ("a request URL", any(x in u for u in r.urls)),
                ("the console", any(x in m for _, m in r.console)),
            )
            if hit
        ]
        if where:
            found.append(f"[smoke] {r.prefix}: password {i + 1} is in " + " and ".join(where))
    return found


def box(ctx, base, out):
    """The box-served page, signed out, then signed in as the engineer."""
    r = Run(open_app(ctx, base, "", layout()), out, "box")
    r.wait(4000)
    r.shot("signed-out")
    extra = []
    # The page asks for a sign-in the moment it opens (docs/help/box.md):
    # the header's form, filled and sent as a person would.
    r.step("signed-in", lambda: extra.extend(dom_sign_in(r, USER, PASSWORD)), HEADER_RIGHT, 1500)
    extra += leaks(r, PASSWORD)
    # The keys work at once after the form goes (docs/help/keyboard.md: a
    # view's key works whenever you are not typing in a field): no click
    # on the canvas first.
    r.step("files", lambda: r.key("Alt+Shift+Digit0"), LEFT, 800)
    r.step("devices", lambda: r.key("Alt+Shift+Digit4"), LEFT, 800)
    # The agent proposes a device profile (this page's session posts it, as
    # the box's tool does with its token); the tab reads the box every 3 s.
    with open(os.path.join(ROOT, "docs", "devices", "moxa-iologik-e1213.device")) as f:
        profile = f.read()
    base = r.page.url.split("?")[0].rstrip("/")
    posted = r.page.request.post(
        base + "/api/device-profile/proposal",
        data=json.dumps({"name": "moxa-copy", "text": profile, "summary": "Read from the unit's manual: eight inputs, four outputs and four that follow a jumper."}),
        headers={"Content-Type": "application/json", "X-Helmsman-Request": "1", "Origin": base},
    )
    if posted.status != 201:
        extra.append(f"the proposal was refused: {posted.status} {posted.text()[:200]}")
    r.wait(4500); r.shot("devices-proposal")
    r.click(*TAB_AGENT); r.wait(800); r.shot("agent-tab")
    r.click(*TAB_IO); r.wait(600); r.shot("io-tab")
    # ? (Shift and /, as a US keyboard types it) opens the Help tab, a
    # box's only (2026-11-26); L shuts the Log that shows, and opens it.
    # (Space asking "Stop the machine?" needs a program running: this box
    # runs none, so box_projects_tests covers it against a real box.)
    r.page.mouse.move(700, 300); r.wait(200)
    r.step("help-tab", lambda: r.key("Shift+Slash"), LEFT, 800)
    # The Help tab opens with the keyboard in its question box, where an L
    # is a letter: a click on the empty diagram gives the view its keys.
    r.click(900, 450); r.wait(300)
    r.step("key-l-log-shut", lambda: r.key("l"), BOTTOM_TABS, 600)
    r.step("key-l-log", lambda: r.key("l"), BOTTOM_TABS, 600)
    # A pane layout kept on the box (this page's engineer session puts it
    # there), then View › Pane layouts…: the popup lists it under "On
    # this box".
    kept = r.page.request.put(
        base + "/api/pane-layouts",
        data=json.dumps({"name": "Commissioning line 3", "left": ["devices", "files", "claude", "code", "history"], "right": ["io"], "left_shows": "devices", "right_shows": "io", "left_open": True, "right_open": True, "bottom_open": True, "bottom_shows": "trend"}),
        headers={"Content-Type": "application/json", "X-Helmsman-Request": "1", "Origin": base},
    )
    if kept.status != 200:
        extra.append(f"the pane layout was refused: {kept.status} {kept.text()[:200]}")
    def layouts():
        r.click(*MENU); r.wait(500)
        r.click(*MENU_VIEW); r.wait(1500)
        r.click(*VIEW_PANE_LAYOUTS_BOX)
    r.step("pane-layouts", layouts, LEFT, 2500)
    r.key("Escape"); r.wait(400)
    # What changes on the machine (docs/UI.md "Show Changes"): a project
    # made through the API opens on a reload, and Deploy (Ctrl/Cmd Shift
    # Enter) opens the sheet against the program the box runs: it checks
    # and replays, and deploys nothing until its own Deploy; Cancel shuts
    # it with nothing deployed.
    hdr = {"Content-Type": "application/json", "X-Helmsman-Request": "1", "Origin": base}
    for method, url, body in (
        ("post", "/api/projects", {"name": "smoke-sheet"}),
        ("put", "/api/projects/smoke-sheet/files/main.st", {"text": SHEET_PROGRAM}),
    ):
        q = getattr(r.page.request, method)(base + url, data=json.dumps(body), headers=hdr)
        if q.status not in (200, 201):
            extra.append(f"{method.upper()} {url} was refused: {q.status} {q.text()[:200]}")
    r.page.reload(wait_until="load"); r.wait(5000)
    r.step("deploy-sheet", lambda: r.key(f"Shift+{MOD}+Enter"), DIAGRAM, 3000)
    r.step("deploy-sheet-cancel", lambda: r.click(*SHEET_CANCEL), DIAGRAM, 1000)
    deployed = [u for u in r.urls if u.endswith("/deploy")]
    if deployed:
        extra.append(f"[smoke] {r.prefix}: the Deploy sheet deployed before its Deploy was pressed: {deployed}")
    problems = extra + r.problems()
    r.page.close()
    # The Monitor by its URL: the header's switch sits elsewhere once
    # somebody is signed in.
    # The Monitor (2026-11-26): the program and its lamp in the header, the
    # device picker first, the Editor's Log and Trend along the bottom.
    m = Run(open_app(ctx, base, "?monitor", layout()), out, "box-monitor")
    m.wait(3000); m.shot("signed-in")
    # The bottom panel's Trend tab, at the page's left on the Monitor.
    m.step("monitor-trend", lambda: m.click(*MONITOR_TREND_TAB), BOTTOM_TABS, 800)
    problems += m.problems()
    m.page.close()
    return problems



# The form's parts, each of which must be there once: a browser's password
# manager finds the fields by these attributes inside a <form>.
LOGIN_PARTS = (
    ("a <form>", "form"),
    ("the user field", "form input[name=username][autocomplete=username]"),
    ("the password field", "form input[type=password][name=password][autocomplete=current-password]"),
    ("a submit button", "form button[type=submit], form input[type=submit]"),
    ("a Cancel button", "form button[type=button]:has-text('Cancel'), form input[type=button][value=Cancel]"),
)


def box_sign_in_form(browser, base, out):
    """The box's sign-in form in the browser, signed out, each check on a
    fresh page: its fields, Esc, Cancel, letters typed in it, the Proposals
    screen's Sign in…, a refusal, then the right password, and no password
    anywhere the page keeps or logs."""
    problems = []

    def fresh(prefix, expect=()):
        ctx = browser.new_context(viewport={"width": W * K, "height": H * K}, device_scale_factor=S, color_scheme="dark")
        r = Run(open_app(ctx, base, "", layout()), out, prefix, expect)
        r.wait(4000)
        return ctx, r

    def gone(r, how):
        try:
            r.page.locator(LOGIN).wait_for(state="detached", timeout=3000)
        except PlaywrightTimeout:
            problems.append(f"[smoke] {r.prefix}: {how} left the sign-in form on the page")

    # Up at load, with every part a password manager needs.
    ctx, r = fresh("box-form")
    r.shot("form")
    if not r.page.locator(LOGIN).is_visible():
        problems.append(f"[smoke] {r.prefix}: no sign-in form ({LOGIN}) on a signed-out box page")
    for what, sel in LOGIN_PARTS:
        n = r.page.locator(LOGIN).locator(sel).count()
        if n != 1:
            problems.append(f"[smoke] {r.prefix}: {what} ({sel}) found {n} times in {LOGIN}")
    url = r.page.url
    # Letters typed into the form are text, never the canvas's view keys:
    # P (Proposals) leaves the screen as it was.
    form = r.page.locator(LOGIN)
    if form.count():
        r.step("p-in-password", lambda: form.locator("input[name=password]").press_sequentially("p"), LEFT, 1200, stays=True)
        r.step("p-in-user", lambda: form.locator("input[name=username]").press_sequentially("pe"), LEFT, 1200, stays=True)
        if form.locator("input[name=password]").input_value() != "p":
            problems.append(f"[smoke] {r.prefix}: the password field does not hold the p typed into it")
        # Esc in the form closes it ...
        form.locator("input[name=username]").press("Escape")
        gone(r, "Esc")
        r.shot("esc")
    if r.page.url != url:
        problems.append(f"[smoke] {r.prefix}: typing in the form changed the URL to {r.page.url}")
    # ... and the view keys are back at once: P opens the Proposals screen.
    r.step("p-after-esc", lambda: r.key("p"), LEFT, 1500)
    problems += r.problems()
    ctx.close()

    # Cancel closes it, and the keys are back.
    ctx, r = fresh("box-cancel")
    if r.page.locator(LOGIN).count():
        r.page.locator(LOGIN + " button:has-text('Cancel')").click()
        gone(r, "Cancel")
    r.shot("cancel")
    r.step("p-after-cancel", lambda: r.key("p"), LEFT, 1500)
    # The header's Sign in brings it back.
    r.step("header-sign-in", lambda: r.click(*HEADER_SIGN_IN), HEADER_RIGHT, 800)
    if not r.page.locator(LOGIN).is_visible():
        problems.append(f"[smoke] {r.prefix}: the header's Sign in did not open the form after Cancel")
    problems += r.problems()
    ctx.close()

    # The Proposals screen's "Sign in…" opens the same form.
    ctx, r = fresh("box-proposals")
    if r.page.locator(LOGIN).count():
        r.page.locator(LOGIN + " button:has-text('Cancel')").click()
        gone(r, "Cancel")
    # The canvas gets the keys from a click on an empty part of it.
    r.click(1000, 400); r.wait(300)
    r.step("proposals", lambda: r.key("p"), LEFT, 1500)
    r.click(*PROPOSALS_SIGN_IN)
    try:
        r.page.locator(LOGIN).wait_for(state="visible", timeout=3000)
    except PlaywrightTimeout:
        problems.append(f"[smoke] {r.prefix}: the Proposals screen's Sign in… did not open {LOGIN}")
    r.shot("proposals-sign-in")
    problems += r.problems()
    ctx.close()

    # A wrong password: the form stays and shows the box's own words; the
    # right one then signs in and the form goes.
    ctx, r = fresh("box-refused")
    form = login_form(r)
    if form is None:
        problems.append(f"[smoke] {r.prefix}: no sign-in form to refuse")
    else:
        form.locator("input[name=username]").fill(USER)
        form.locator("input[name=password]").fill(WRONG_PASSWORD)
        with r.page.expect_response(lambda q: q.url.endswith("/api/login") and q.request.method == "POST", timeout=15000) as answer:
            form.locator("button[type=submit]").click()
        a = answer.value
        r.expect += (f"status of {a.status}",)
        try:
            words = json.loads(a.text()).get("error") or a.text()
        except ValueError:
            words = a.text()
        line = r.page.locator(LOGIN + " p.problem[role=alert]")
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and not (line.count() and line.inner_text().strip()):
            r.wait(100)
        r.shot("wrong-password")
        if a.status < 400:
            problems.append(f"[smoke] {r.prefix}: the box took a wrong password ({a.status})")
        elif not r.page.locator(LOGIN).is_visible():
            problems.append(f"[smoke] {r.prefix}: a refused sign-in closed the form")
        elif not line.count():
            problems.append(f"[smoke] {r.prefix}: a refused sign-in shows no p.problem[role=alert] (the form says {r.page.locator(LOGIN).inner_text()!r})")
        elif words.strip() not in line.inner_text():
            problems.append(f"[smoke] {r.prefix}: the form says {line.inner_text()!r}, the box said {words!r}")
        problems += dom_sign_in(r, USER, PASSWORD)
        r.wait(1500)
        r.shot("signed-in")
        problems += leaks(r, PASSWORD, WRONG_PASSWORD)
        # Signed in, a reload shows no form: the session holds.
        r.page.reload(wait_until="load"); r.wait(4000)
        if r.page.locator(LOGIN).count():
            problems.append(f"[smoke] {r.prefix}: a signed-in page shows the sign-in form after a reload")
        r.shot("reloaded")
        problems += leaks(r, PASSWORD, WRONG_PASSWORD)
    problems += r.problems()
    ctx.close()
    return problems

# ---- the marketplace install and the Agent's review --------------------------

BENIGN = {
    "summary": "A horn that sounds while the start button is held. It reads start and drives horn, as its description says.",
    "reads": ["start: the push button"],
    "writes": ["horn: on while start is held, off at once when it is released"],
    "devices": [], "access": [], "at_start": [], "undeclared": [],
    "risk": "medium", "refuse": False,
    "reason": "Check that the horn is wired to the output called horn before you approve.",
}
RISKY = dict(
    BENIGN,
    summary="A siren that, once started, stays on. Nothing in the program turns it off.",
    writes=["horn: set when start is pressed and never cleared"],
    undeclared=["the output is latched with no way to clear it"],
    risk="high", refuse=True,
    reason="It latches the horn on and nothing turns it off, which could hurt someone standing near it. <b>Do not install.</b>",
)


def relay_events(text):
    def ev(d):
        return f"event: {d['type']}\ndata: {json.dumps(d)}\n\n"
    return (
        ev({"type": "message_start", "message": {"id": "m", "type": "message", "role": "assistant", "content": [], "model": "m", "stop_reason": None, "usage": {"input_tokens": 1, "output_tokens": 1}}})
        + ev({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})
        + ev({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}})
        + ev({"type": "content_block_stop", "index": 0})
        + ev({"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": None}, "usage": {"output_tokens": 9}})
        + ev({"type": "message_stop"})
        + ev({"type": "relay_usage", "request_id": "r", "agent_version": "helmsman/review-v1", "model": "m", "input_tokens": 1, "output_tokens": 9, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0})
    )


class Relay(http.server.BaseHTTPRequestHandler):
    """The Agent, scripted: refuses the bundle whose source says RISKY."""

    def log_message(self, *_args):
        pass

    def do_GET(self):
        body = b"{}"
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        n = int(self.headers.get("Content-Length", "0"))
        asked = self.rfile.read(n).decode()
        verdict = RISKY if "RISKY" in asked else BENIGN
        body = relay_events(json.dumps(verdict)).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)


def plc(*args, env=None, **kw):
    return subprocess.run([PLC, *args], env=env, text=True, capture_output=True, check=True, **kw)


def make_market(work):
    """A signed index of two bundles in work/site, the publisher's key, and
    the module and map of the box that installs from it."""
    os.makedirs(os.path.join(work, "site"))
    key = os.path.join(work, "acme.key")
    out = plc("bundle", "keygen", key, "--name", "acme").stdout
    trust_key = out.split("trust add acme ")[1].split()[0]
    for name, desc, src in (
        ("horn-kit", "Sounds the horn while start is held.", "  (* Horn follows the start button *)\n  horn := start;\n"),
        ("siren-kit", "A siren for the start button.", "  (* RISKY: latched, never cleared *)\n  IF start THEN horn := TRUE; END_IF;\n"),
    ):
        d = os.path.join(work, name)
        os.makedirs(d)
        with open(os.path.join(d, "module.manifest"), "w") as f:
            f.write(f"name     {name.replace('-kit', '')}\nauthor   Acme Automation\nscan_ms  10\nreads    start\ndrives   horn\n")
        with open(os.path.join(d, "module.st"), "w") as f:
            f.write(f"PROGRAM {name.replace('-kit', '')}\nVAR_INPUT start : BOOL; END_VAR\nVAR_OUTPUT horn : BOOL; END_VAR\n{src}END_PROGRAM\n")
        bundle = os.path.join(work, "site", f"{name}-1.0.0.hbundle")
        plc("bundle", "pack", "--module-dir", d, "--name", name, "--version", "1.0.0", "--author", "Acme Automation", "--description", desc, bundle)
        plc("bundle", "sign", "--key", key, bundle, bundle)
    site = os.path.join(work, "site")
    plc("bundle", "index", "--key", key, "--publisher", "acme", site, os.path.join(site, "index.json"))
    conv = os.path.join(work, "conveyor")
    os.makedirs(conv)
    with open(os.path.join(conv, "module.manifest"), "w") as f:
        f.write("name     conveyor\nauthor   Test\nscan_ms  10\nreads    start\ndrives   lamp\n")
    with open(os.path.join(conv, "module.st"), "w") as f:
        f.write("PROGRAM conveyor\nVAR_INPUT start : BOOL; END_VAR\nVAR_OUTPUT lamp : BOOL; END_VAR\n  lamp := start;\nEND_PROGRAM\n")
    with open(os.path.join(work, "map.txt"), "w") as f:
        f.write("in  start bool box di:0\nout lamp  bool box do:0 safe=false\nout horn  bool box do:1 safe=false\n")
    return trust_key


def start_market_box(dist, work, trust_key, port, relay_port):
    """A box that installs from the loopback marketplace; the relay (or
    none, with relay_port None)."""
    state = os.path.join(work, f"state-{port}")
    os.makedirs(state)
    env = dict(os.environ, STATE_DIRECTORY=state)
    plc("account", "add", USER, "engineer", env=env, input=PASSWORD + "\n")
    plc("bundle", "trust", "--state-dir", state, "add", "acme", trust_key)
    args = [PLC, "--module-dir", os.path.join(work, "conveyor"), "--io-map", os.path.join(work, "map.txt"),
            "--api", f"127.0.0.1:{port}", "--https", "off", "--opcua", "off", "--ui", dist, "--bind", "127.0.0.1",
            "--encap-port", "0", "--io-port", "0", "--market-allow-local", "on"]
    if relay_port:
        token = os.path.join(state, "relay-token")
        with open(token, "w") as f:
            f.write("hmr1_" + "0123456789abcdef" * 4 + "\n")
        os.chmod(token, 0o600)
        args += ["--relay", f"http://127.0.0.1:{relay_port}", "--relay-token", token]
    proc = subprocess.Popen(args, env=env, cwd=state, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    if not wait_for(f"http://127.0.0.1:{port}/api"):
        proc.kill()
        sys.exit("the market box did not answer: " + (proc.stderr.read() or "")[:500])
    return proc


# On the Proposals screen at 1400x860: the install form is under the drafts
# in the left panel (crates/helmsman-canvas/src/review_view.rs).


def market(browser, dist, out):
    """The Proposals screen's install form, against a box with a relay
    that reviews, and against one without a relay."""
    problems = []
    work = tempfile.mkdtemp(prefix="helmsman-market-")
    srv = relay = rproc = bproc = None
    try:
        trust_key = make_market(work)
        site = serve(os.path.join(work, "site"))
        index = f"http://127.0.0.1:{site.server_address[1]}/index.json"
        relay = socketserver.ThreadingTCPServer(("127.0.0.1", 0), Relay)
        relay.daemon_threads = True
        threading.Thread(target=relay.serve_forever, daemon=True).start()
        for label, relay_port in (("review", relay.server_address[1]), ("noreview", None)):
            port = free_port()
            proc = start_market_box(dist, work, trust_key, port, relay_port)
            try:
                ctx = browser.new_context(viewport={"width": W * K, "height": H * K}, device_scale_factor=S, color_scheme="dark")
                base = f"http://127.0.0.1:{port}"
                r = Run(open_app(ctx, base, "", layout()), out, f"market-{label}")
                r.wait(4000)
                problems += dom_sign_in(r, USER, PASSWORD); r.wait(1500)
                r.click(*MENU); r.wait(500)
                r.click(*MENU_VIEW); r.wait(1500)
                r.click(*VIEW_PROPOSALS_BOX); r.wait(1500)
                r.shot("proposals-screen")
                problems += install_steps(r, index, label)
                problems += r.problems()
                ctx.close()
            finally:
                proc.terminate()
                try:
                    proc.wait(5)
                except subprocess.TimeoutExpired:
                    proc.kill()
    finally:
        for s in (relay,):
            if s:
                s.shutdown()
        shutil.rmtree(work, ignore_errors=True)
    return problems


def fill(r, field, text):
    r.click(*field); r.key(f"{MOD}+a"); r.page.keyboard.type(text); r.wait(200)


INSTALL_INDEX_FIELD = (133, 312)
INSTALL_NAME_FIELD = (133, 374)
INSTALL_BUTTON = (42, 410)


def install_steps(r, index, label):
    """Install horn-kit and siren-kit through the form, a screenshot each."""
    fill(r, INSTALL_INDEX_FIELD, index)
    fill(r, INSTALL_NAME_FIELD, "horn-kit")
    r.click(*INSTALL_BUTTON); r.wait(4000)
    r.shot("horn-kit")
    if label == "review":
        # The first install left a draft in the list: the form is lower.
        r.click(330, 138); r.wait(300)
        fill(r, (133, 374), index)
        fill(r, (133, 435), "siren-kit")
        r.click(42, 462); r.wait(4000)
        r.shot("siren-kit-refused")
    else:
        r.page.mouse.move(395, 262); r.wait(400)
        r.click(395, 262); r.wait(3000)
        r.shot("went-on-without-a-review")
    return []


def phone(browser, base, query, out, prefix):
    ctx = browser.new_context(viewport={"width": 390 * K, "height": 844 * K}, device_scale_factor=S, color_scheme="dark")
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


# What the loading screen says without WebGL2 (loading/loading.js, "nowebgl").
NO_WEBGL2_NOTICE = "This browser cannot draw Helmsman"

NO_WEBGL2_INIT = """
const get = HTMLCanvasElement.prototype.getContext;
HTMLCanvasElement.prototype.getContext = function (kind, ...rest) {
    return kind === 'webgl2' ? null : get.call(this, kind, ...rest);
};
"""

PROBE = """() => {
    const gl = document.createElement('canvas').getContext('webgl2');
    if (!gl) return null;
    const ext = gl.getExtension('WEBGL_debug_renderer_info');
    return ext ? gl.getParameter(ext.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.VERSION);
}"""


def launch(p, engine, nogpu):
    """The engine's browser, with software drawing forced when `nogpu`."""
    if engine == "chromium":
        args = ["--disable-gpu", "--use-angle=swiftshader", "--enable-unsafe-swiftshader", "--ignore-gpu-blocklist"] if nogpu else []
        return p.chromium.launch(args=args)
    if engine == "firefox":
        # Without a GPU (a CI runner) Firefox's blocklist leaves WebGL2 off
        # and the page shows its notice: force it on in both modes, and in
        # software mode draw in software as well.
        prefs = {"webgl.force-enabled": True, "webgl.disabled": False}
        if nogpu:
            prefs.update({"layers.acceleration.disabled": True, "gfx.webrender.software": True})
        return p.firefox.launch(firefox_user_prefs=prefs, headless=not HEADED)
    return p.webkit.launch()


PROBE_APP = """() => {
    const c = document.getElementById('canvas');
    const r = c.getBoundingClientRect();
    return `dpr ${devicePixelRatio} inner ${innerWidth}x${innerHeight} canvas buffer ${c.width}x${c.height} css ${r.width}x${r.height}`;
}"""

PROBE_GL1 = """() => {
    const gl = document.createElement('canvas').getContext('webgl');
    if (!gl) return null;
    const ext = gl.getExtension('WEBGL_debug_renderer_info');
    return ext ? gl.getParameter(ext.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.VERSION);
}"""


def probe_webgl2(browser, base):
    """One line: the WebGL2 and WebGL1 renderers the engine gives a page, and
    the size of the app's canvas against its CSS size at this engine's layout."""
    page = browser.new_page()
    page.goto(base + "/__seed", wait_until="load")
    gl2, gl1 = page.evaluate(PROBE), page.evaluate(PROBE_GL1)
    page.close()
    ctx = browser.new_context(viewport={"width": W * K, "height": H * K}, device_scale_factor=S)
    page = open_app(ctx, base, "?playground&editor", layout())
    page.wait_for_timeout(3000)
    metrics = page.evaluate(PROBE_APP)
    ctx.close()
    return f"WebGL2 {gl2!r}, WebGL1 {gl1!r}, {metrics} (viewport {W * K}x{H * K})"


def no_webgl2(browser, base, out):
    """With getContext('webgl2') answering null the page shows its notice."""
    ctx = browser.new_context(viewport={"width": W * K, "height": H * K}, device_scale_factor=S, color_scheme="dark")
    ctx.add_init_script(NO_WEBGL2_INIT)
    r = Run(open_app(ctx, base, "?playground&editor", layout()), out, "nowebgl")
    r.wait(4000)
    r.shot("notice")
    problems = r.problems()
    shown = r.page.locator("#loading-why").inner_text() if r.page.locator("#loading-why").count() else ""
    if not r.page.locator("#loading-failed").is_visible() or NO_WEBGL2_NOTICE not in shown:
        problems.append(f"[smoke] no WebGL2: the notice did not show (page said {shown!r})")
    else:
        print(f"[smoke] no WebGL2 notice: {shown}")
    ctx.close()
    return problems


def run_engine(browser, base, out, a, proc, box_port):
    """Every scenario in one launched browser: its problems."""
    if a.market_only:
        return market(browser, a.dist, out)
    problems = []
    ctx = browser.new_context(viewport={"width": W * K, "height": H * K}, device_scale_factor=S, color_scheme="dark")
    problems += playground(ctx, base, out)
    ctx.close()
    problems += first_visit(browser, base, out)
    problems += phone(browser, base, "?playground&editor", out, "pg")
    problems += no_webgl2(browser, base, out)
    if proc:
        bbase = f"http://127.0.0.1:{box_port}"
        ctx = browser.new_context(viewport={"width": W * K, "height": H * K}, device_scale_factor=S, color_scheme="dark")
        problems += box(ctx, bbase, out)
        ctx.close()
        problems += box_sign_in_form(browser, bbase, out)
        problems += phone(browser, bbase, "", out, "box")
        problems += market(browser, a.dist, out)
    return problems


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--dist", default=os.path.join(ROOT, "target", "canvas-dist"))
    ap.add_argument("--out", default=os.path.join(ROOT, "target", "canvas-smoke"))
    ap.add_argument("--box", action="store_true", help="also start a loopback box and drive its page")
    ap.add_argument("--market-only", action="store_true", help="only the marketplace install scenario (starts its own boxes)")
    ap.add_argument("--browser", default="chromium", choices=("chromium", "firefox", "webkit", "all"))
    ap.add_argument("--no-gpu", action="store_true", help="force software drawing (with --browser all both modes run regardless)")
    ap.add_argument("--headed", action="store_true", help="Firefox with a window (xvfb-run on a runner), for a GL context headless lacks")
    a = ap.parse_args()
    global PLC, HEADED
    HEADED = a.headed
    PLC = os.path.join(ROOT, "target", "debug", "helmsman-plc")
    if not os.path.exists(os.path.join(a.dist, "index.html")):
        sys.exit(f"no canvas build in {a.dist}: make canvas-web")
    os.makedirs(a.out, exist_ok=True)
    srv = serve(a.dist)
    base = f"http://127.0.0.1:{srv.server_address[1]}"
    problems = []
    proc = None
    state = tempfile.mkdtemp(prefix="helmsman-smoke-")
    try:
        if a.box and not a.market_only:
            box_port = free_port()
            proc = start_box(a.dist, state, box_port)
        with sync_playwright() as p:
            every = a.browser == "all"
            for engine in (("chromium", "firefox", "webkit") if every else (a.browser,)):
                for nogpu in ((False, True) if every else (a.no_gpu,)):
                    name = engine + ("-nogpu" if nogpu else "")
                    out = os.path.join(a.out, name) if every or a.no_gpu or engine != "chromium" else a.out
                    os.makedirs(out, exist_ok=True)
                    global K
                    K = 1 if engine == "webkit" else 2
                    browser = launch(p, engine, nogpu)
                    print(f"[smoke] {name}: {probe_webgl2(browser, base)}", flush=True)
                    found = [f"{name}: {x}" for x in run_engine(browser, base, out, a, proc, box_port if proc else None)]
                    problems += found
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
    shots = [f for _, _, fs in os.walk(a.out) for f in fs if f.endswith(".png")]
    print(f"{len(shots)} screenshots in {a.out}")
    if problems:
        print(f"{len(problems)} console problem(s):")
        for x in problems:
            print("  " + x)
        sys.exit(1)
    print("no console errors or panics")


if __name__ == "__main__":
    main()
