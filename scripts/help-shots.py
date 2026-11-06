"""The help pages' pictures, taken from the canvas in a headless browser.

usage: python3 -I scripts/help-shots.py [dist dir] [out dir] [names...]
       make help-shots            (after make canvas-web)

Needs Playwright's Chromium for python3 (pip install playwright;
python3 -m playwright install chromium). Serves <dist dir>
(target/canvas-dist) on 127.0.0.1:8794 and drives ?playground: the app
draws at S CSS px a point on a doubled viewport at DPR 2 but maps the
pointer one to one, so clicks are in points and the PNGs are halved to
2x pictures of the 1400x860 layout. The remembered layout (localStorage
"helmsman-canvas/editor") is written from a same-origin page that is not
the app, before the app loads (eframe saves the app's own layout on
beforeunload). Each shot may press keys or click, then writes
<out>/<name>.png, clipped when a region is given; the narrow ones are
shown at half their pixel width by the pages (width= on the img).
"""

import http.server
import os
import socketserver
import sys
import threading

from PIL import Image
from playwright.sync_api import sync_playwright

PORT = 8794
KEY = "helmsman-canvas/editor"
W, H = 1400, 860
S = 2


def layout(left=1, right=1, pane="code", tab="io", scan=0, trend=1, bottom="trend", light=0):
    return f"left={left} right={right} pane={pane} tab={tab} scan={scan} trend={trend} bottom={bottom} light={light}"


def clip(x, y, w, h):
    return {"x": x * S, "y": y * S, "width": w * S, "height": h * S}


# name: (query, layout string, steps, clip or None); points, not pixels.
# a step is ("key", "Control+Enter") | ("click", x, y) | ("rclick", x, y) | ("move", x, y) | ("wait", ms)
SHOTS = {
    "editor": ("?playground", layout(), [("wait", 2500)], None),
    "editor-light": ("?playground", layout(light=1), [("wait", 2500)], None),
    "file-menu": ("?playground", layout(), [("click", 137, 26), ("wait", 800)], clip(0, 0, 520, 300)),
    "glyphs": ("?playground", layout(), [("move", 838, 108), ("wait", 1200)], clip(800, 80, 300, 70)),
    "diagram-why": ("?playground", layout(), [("click", 731, 242), ("wait", 1500)], clip(262, 75, 836, 340)),
    "files-pane": ("?playground", layout(pane="files"), [("wait", 1500)], clip(0, 75, 360, 470)),
    "context-menu": ("?playground", layout(pane="files"), [("wait", 1500), ("rclick", 55, 400), ("wait", 800)], clip(0, 75, 360, 470)),
    "io-tab": ("?playground", layout(tab="io"), [("wait", 1500)], clip(1098, 75, 302, 400)),
    "agent-tab": ("?playground", layout(tab="claude"), [("wait", 1500)], clip(1098, 75, 302, 370)),
    "trend": ("?playground", layout(bottom="trend"), [("wait", 8000)], clip(262, 660, 836, 200)),
    "monitor": ("?playground&monitor", layout(), [("wait", 3000)], None),
    "monitor-stopped": ("?playground&monitor", layout(), [("wait", 2000), ("key", "Control+Enter"), ("wait", 1200)], clip(0, 52, 700, 260)),
}


def serve(root):
    def handler(*a, **k):
        return http.server.SimpleHTTPRequestHandler(*a, directory=root, **k)

    socketserver.TCPServer.allow_reuse_address = True
    srv = socketserver.TCPServer(("127.0.0.1", PORT), handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def main():
    args = sys.argv[1:]
    dist = args[0] if args else "target/canvas-dist"
    out = args[1] if len(args) > 1 else "docs/help/img"
    names = args[2:] or list(SHOTS)
    os.makedirs(out, exist_ok=True)
    srv = serve(dist)
    try:
        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True)
            for name in names:
                query, lay, steps, region = SHOTS[name]
                scheme = "light" if "light=1" in lay else "dark"
                ctx = browser.new_context(
                    viewport={"width": W * S, "height": H * S}, device_scale_factor=S, color_scheme=scheme
                )
                page = ctx.new_page()
                page.goto(f"http://127.0.0.1:{PORT}/nothing-here.txt")
                page.evaluate("([k, v]) => localStorage.setItem(k, v)", [KEY, lay])
                page.goto(f"http://127.0.0.1:{PORT}/{query}")
                page.wait_for_timeout(4000)
                # The app draws at S CSS px a point but maps the pointer
                # one to one, so clicks are in points, unscaled.
                page.mouse.move(700, 400)
                for step in steps:
                    if step[0] == "key":
                        page.keyboard.press(step[1])
                    elif step[0] == "click":
                        page.mouse.click(step[1], step[2])
                    elif step[0] == "rclick":
                        page.mouse.click(step[1], step[2], button="right")
                    elif step[0] == "move":
                        page.mouse.move(step[1], step[2])
                    elif step[0] == "wait":
                        page.wait_for_timeout(step[1])
                    page.wait_for_timeout(200)
                path = os.path.join(out, f"{name}.png")
                if region:
                    page.screenshot(path=path, clip=region)
                else:
                    page.screenshot(path=path)
                # DPR 2 on a doubled viewport is 4 px a point: halve it
                # to a 2x image of the 1400x860 layout.
                im = Image.open(path)
                im = im.resize((im.width // 2, im.height // 2), Image.LANCZOS)
                im.save(path, optimize=True)
                print(name, path, im.size, os.path.getsize(path) // 1024, "KB", flush=True)
                ctx.close()
            browser.close()
    finally:
        srv.shutdown()


if __name__ == "__main__":
    main()
