"""A smoke of the live public page: what a visitor gets today.

usage: python3 -I scripts/live-smoke.py [--site URL] [--out DIR]
       make live-smoke     (after a promotion to master)

Opens SITE/playground/ in headless Chromium as a first visit (nothing kept
in the browser), at a laptop's size and a phone's, and fails when:
  - the page logs an error or panics,
  - nothing is drawn (the canvas is one flat colour),
  - the front door's machine does not move (its picture is the same 4 s
    apart),
  - "Open the editor" changes nothing.
A screenshot per step goes to target/live-smoke/ to look at. Reads only;
it sends nothing to the Agent, so it costs no model turn.

Needs Playwright's Chromium for python3 (as scripts/canvas-smoke.py).
"""
import argparse
import hashlib
import os
import sys

from playwright.sync_api import sync_playwright

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# In points at the 1400 x 860 window (crates/helmsman-canvas/src/front.rs):
# the machine's picture in the middle card, and "Open the editor".
MACHINE = (500, 230, 400, 300)
OPEN_EDITOR = (1264, 33)
# Console noise that is not the page's fault.
EXPECTED = ("favicon",)


def digest(page, region=None):
    clip = None
    if region:
        x, y, w, h = region
        clip = {"x": x, "y": y, "width": w, "height": h}
    return hashlib.sha256(page.screenshot(clip=clip)).hexdigest()


def flat(page):
    """Whether the page is one colour: nothing was drawn. A screenshot of a
    flat page compresses to a few kilobytes; a drawn one does not. (The
    canvas itself cannot be read back: it is WebGL without a kept buffer.)"""
    return len(page.screenshot()) < 12_000


def visit(browser, url, out, name, size):
    problems, console = [], []
    ctx = browser.new_context(viewport={"width": size[0], "height": size[1]}, device_scale_factor=1, color_scheme="dark")
    page = ctx.new_page()
    page.on("console", lambda m: console.append((m.type, m.text)))
    page.on("pageerror", lambda e: console.append(("pageerror", str(e))))
    resp = page.goto(url, wait_until="load")
    if resp is None or resp.status != 200:
        ctx.close()
        return [f"[{name}] {url} answered {resp.status if resp else 'nothing'}"]
    page.wait_for_timeout(9000)
    page.screenshot(path=os.path.join(out, f"{name}-1-first-visit.png"))
    if flat(page):
        problems.append(f"[{name}] nothing is drawn: the page is one flat colour")
    if name == "laptop":
        before = digest(page, MACHINE)
        page.wait_for_timeout(4000)
        if digest(page, MACHINE) == before:
            problems.append(f"[{name}] the front door's machine is not moving")
        whole = digest(page)
        page.mouse.click(*OPEN_EDITOR)
        page.wait_for_timeout(2500)
        page.screenshot(path=os.path.join(out, f"{name}-2-editor.png"))
        if digest(page) == whole:
            problems.append(f"[{name}] 'Open the editor' changed nothing")
    problems += [
        f"[{name}] [{t}] {x[:300]}"
        for (t, x) in console
        if t in ("error", "pageerror") and not any(e in x for e in EXPECTED)
    ]
    ctx.close()
    return problems


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--site", default=os.environ.get("HELMSMAN_SITE", "https://helmsman.advin.io"))
    ap.add_argument("--out", default=os.path.join(ROOT, "target", "live-smoke"))
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    url = a.site.rstrip("/") + "/playground/"
    problems = []
    with sync_playwright() as p:
        browser = p.chromium.launch()
        problems += visit(browser, url, a.out, "laptop", (1400, 860))
        problems += visit(browser, url, a.out, "phone", (390, 844))
        browser.close()
    print(f"screenshots in {a.out}")
    if problems:
        print("\n".join(problems))
        sys.exit(1)
    print(f"{url}: drawn, moving, no console errors")


if __name__ == "__main__":
    main()
