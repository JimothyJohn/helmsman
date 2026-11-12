"""Record the playground running the Start/stop latch example, for the ad.

usage: python3 -I marketing/helmsman-ad/tools/capture.py <docs dir> <work dir> [probe]

Serves <docs dir> (docs/, which holds the published playground) on
127.0.0.1:8796 and drives /playground/ in Playwright's headless Chromium,
the way scripts/help-shots.py does. The app draws at DPR squared pixels
a point, so a 1920x1080 viewport at DPR 1.5 is the 1280x720 point layout
in 2880x1620 frames; the pointer is in points, and the remembered layout
goes into localStorage from a same-origin page before the app loads.

It opens the example the way a user does (Files, latch, a name), shows
the code, drags the trend panel taller, then plays TAKE: run, hold start,
hold stop.
Frames come from Chromium's screencast, each with its own time; <work dir> gets
frames/NNNN.jpg, frames.txt (an ffmpeg concat list with each frame's real
duration) and events.json (when each action happened, in seconds from the
first frame, and where, in points). `probe` takes one PNG and stops.

Then, to make the clip the composition plays:
  ffmpeg -f concat -safe 0 -i <work>/frames.txt -vf fps=30 \
    -c:v libx264 -crf 14 -pix_fmt yuv420p -an marketing/helmsman-ad/assets/app-latch.mp4
"""

import base64
import http.server
import json
import os
import socketserver
import sys
import threading
import time

from playwright.sync_api import sync_playwright

PORT = 8796
KEY = "helmsman-canvas/editor"
W, H = 1280, 720  # points
DPR = 1.5
LAYOUT = "left=1 right=1 pane=files tab=io scan=0 trend=1 bottom=trend light=0"

# Points in the 1280x720 layout.
LATCH = (65, 253)  # "latch" under Examples
CODE_TAB = (183, 98)
RUN = (717, 109)
START = (332, 240)
STOP = (332, 360)
REST = (1000, 300)  # empty diagram: no tooltip
# The trend panel's top edge (grabbed a point below it) and where it is
# dragged to, just under the diagram, so each signal gets a readable row.
TREND_EDGE = (900, 521)
TREND_TOP = 396

# (seconds, action, point)
TAKE = [
    (1.0, "move", RUN),
    (1.3, "click", RUN),
    (2.9, "move", START),
    (3.2, "down", START),
    (3.8, "up", START),
    (3.9, "move", REST),
    (6.3, "move", STOP),
    (6.6, "down", STOP),
    (7.2, "up", STOP),
    (7.3, "move", REST),
]
LENGTH = 10.0


def serve(root):
    def handler(*a, **k):
        return http.server.SimpleHTTPRequestHandler(*a, directory=root, **k)

    handler_cls = handler
    socketserver.TCPServer.allow_reuse_address = True
    srv = socketserver.TCPServer(("127.0.0.1", PORT), handler_cls)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def act(page, kind, point):
    x, y = point
    if kind == "move":
        page.mouse.move(x, y)
    elif kind == "click":
        page.mouse.click(x, y)
    elif kind == "down":
        page.mouse.move(x, y)
        page.mouse.down()
    elif kind == "up":
        page.mouse.up()


def main():
    docs, work = sys.argv[1], sys.argv[2]
    probe = len(sys.argv) > 3 and sys.argv[3] == "probe"
    frames = os.path.join(work, "frames")
    os.makedirs(frames, exist_ok=True)
    srv = serve(docs)
    try:
        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True)
            ctx = browser.new_context(
                viewport={"width": int(W * DPR), "height": int(H * DPR)}, device_scale_factor=DPR, color_scheme="dark"
            )
            page = ctx.new_page()
            page.goto(f"http://127.0.0.1:{PORT}/nothing-here.txt")
            page.evaluate("([k, v]) => localStorage.setItem(k, v)", [KEY, LAYOUT])
            page.goto(f"http://127.0.0.1:{PORT}/playground/")
            page.wait_for_timeout(4000)
            page.mouse.move(*REST)
            page.wait_for_timeout(800)
            page.mouse.click(*LATCH)
            page.wait_for_timeout(1000)
            page.keyboard.press("Meta+a")
            page.keyboard.type("conveyor")
            page.keyboard.press("Enter")
            page.wait_for_timeout(1500)
            page.mouse.click(*CODE_TAB)
            page.wait_for_timeout(600)
            x, y = TREND_EDGE
            page.mouse.move(x, y - 10)
            page.mouse.move(x, y)
            page.wait_for_timeout(400)
            page.mouse.down()
            for step in range(1, 6):
                page.mouse.move(x, y + (TREND_TOP - y) * step / 5)
            page.mouse.up()
            page.wait_for_timeout(600)
            page.mouse.move(*REST)
            page.wait_for_timeout(1200)
            if probe:
                page.screenshot(path=os.path.join(work, "probe.png"))
                print("probe", os.path.join(work, "probe.png"))
                return
            # Chromium's screencast pushes each frame it paints, far faster
            # than a screenshot a frame; a frame carries its own wall-clock
            # time, which is the clock the events are stamped on too.
            todo = list(TAKE)
            events, stamps = [], []
            cdp = ctx.new_cdp_session(page)

            def on_frame(params):
                stamps.append(params["metadata"]["timestamp"])
                with open(os.path.join(frames, f"{len(stamps) - 1:04d}.jpg"), "wb") as f:
                    f.write(base64.b64decode(params["data"]))
                cdp.send("Page.screencastFrameAck", {"sessionId": params["sessionId"]})

            cdp.on("Page.screencastFrame", on_frame)
            cdp.send(
                "Page.startScreencast",
                {"format": "jpeg", "quality": 95, "maxWidth": int(W * DPR * DPR), "maxHeight": int(H * DPR * DPR)},
            )
            t0 = time.monotonic()
            while time.monotonic() - t0 < LENGTH:
                while todo and todo[0][0] <= time.monotonic() - t0:
                    _, kind, point = todo.pop(0)
                    act(page, kind, point)
                    events.append({"t": time.time(), "action": kind, "x": point[0], "y": point[1]})
                page.wait_for_timeout(10)
            cdp.send("Page.stopScreencast")
            end = stamps[-1] + 1 / 30
            n = len(stamps)
            browser.close()
        first = stamps[0]
        with open(os.path.join(work, "frames.txt"), "w") as f:
            for i, at in enumerate(stamps):
                nxt = stamps[i + 1] if i + 1 < len(stamps) else end
                f.write(f"file 'frames/{i:04d}.jpg'\nduration {nxt - at:.4f}\n")
            f.write(f"file 'frames/{len(stamps) - 1:04d}.jpg'\n")
        for e in events:
            e["t"] = round(e["t"] - first, 3)
        with open(os.path.join(work, "events.json"), "w") as f:
            json.dump({"frames": n, "seconds": round(end - first, 3), "events": events}, f, indent=1)
        print(n, "frames in", round(end - first, 2), "s:", round(n / (end - first), 1), "fps")
        for e in events:
            print(e)
    finally:
        srv.shutdown()


if __name__ == "__main__":
    main()
