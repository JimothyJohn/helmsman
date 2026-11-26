"""Record the playground doing each benefit, for the benefit films.

usage: python3 -I marketing/helmsman-benefits/tools/record.py <site dir> <work dir> [scene ...] [--probe]

Serves <site dir> (a directory holding playground/, as built by
scripts/build-playground.sh target/film-site/playground /playground/) on
127.0.0.1:8790, the loopback origin the relay's free chat accepts (so the
Describe tab works, as in scripts/canvas-serve.sh), and drives
/playground/ in Playwright's headless Chromium the way
../helmsman-ad/tools/capture.py does: a 1920x1080 viewport at DPR 1.5 is
the 1280x720 point layout in 2880x1620 frames; every point below is in
that layout.

Each scene opens the page, runs its SETUP off camera, then plays its TAKE
on a clock while Chromium's screencast keeps every frame it paints. For a
scene <work dir>/<scene>/ gets frames/, frames.txt (an ffmpeg concat list
with each frame's real duration) and events.json (each action, in
seconds from the first frame, with its point), and the clip is encoded to
marketing/helmsman-benefits/assets/app/<scene>.mp4 at 30 fps with its
events beside it as <scene>.json. The cursor is not in the frames: the
films draw it from those events.

--probe runs SETUP and TAKE without recording and leaves a PNG after each
TAKE step in <work dir>/<scene>/probe/, for finding points.

Needs Playwright's Chromium for python3 and FFmpeg.
"""

import base64
import http.server
import json
import os
import shutil
import socketserver
import subprocess
import sys
import threading
import time

from playwright.sync_api import sync_playwright

PORT = 8790
KEY = "helmsman-canvas/editor"
W, H = 1280, 720  # points
DPR = 1.5
HERE = os.path.dirname(os.path.abspath(__file__))
FILM = os.path.dirname(HERE)
UPLOADS = os.path.join(FILM, "uploads")
ZIPS = os.path.join(os.environ.get("TMPDIR", "/tmp"), "helmsman-benefits-zips")

LAYOUT = "left=1 right=1 pane=files tab=io scan=0 trend=1 bottom=trend light=0"
REST = (640, 12)  # the header: no tooltip anywhere

# Points in the 1280x720 layout, read off probe shots of the dev build (2026-11-25).
MENU = (181, 26)
FILE = (190, 63)
NEW_PROJECT = (320, 67)
FILES_TAB = (188, 71)
DESCRIBE_TAB = (24, 71)
CODE_TAB = (65, 71)
IO_TAB = (106, 71)
DEVICES_TAB = (147, 71)
UPLOAD = (1207, 83)
DOWNLOAD = (1250, 83)
RUN = (975, 83)
DESCRIBE_FIELD = (154, 125)  # a new project: no "Name the I/O first" fold above it
DESCRIBE_SEND = (266, 166)
# Under the Describe field once a project has versions: the clock (the list), back, forward.
VERSIONS = (40, 166)
BACK = (115, 166)
FORWARD = (189, 166)
MARY_SWITCH = (400, 188)  # iologik_e1213_di_00 on the diagram, top left
LAMP_INPUT = (413, 237)  # the input's switch in the answer the probes got (one INPUT, one LAMP)
SCAN = (177, 193)  # Devices tab: Scan, beside "On the network"
ADD_FOUND = (58, 330)  # Add, under the first device the scan found
ADD_CONFIRM = (36, 554)  # Add, in the "Add Moxa ioLogik E1213" box it opens
LAMP_BLOCK = (512, 170)  # the uploaded program's LAMP := on the diagram
EMPTY_DIAGRAM = (1100, 140)
ZOOM_IN = (975, 370)
INT_WORD = (183, 285)  # Code tab, pane widened: INT in "enable : INT;", line 7
PANE_EDGE = (316, 400)  # the left pane's right edge
FAIL_HIGH = (544, 687)  # front door, under Break it: "Fail the high switch"
OPEN_EDITOR = (1184, 32)  # front door, top right
EXAMPLES = (61, 285)  # the Examples heading, with one project of mine open
WINDOW_KNOB = (927, 450)  # the trend window slider at 30 s, while running


def new_project(name):
    """File > New project..., named; the menu is crossed sideways so the submenu holds."""
    return [
        ("click", MENU),
        ("click", FILE),
        ("move", (300, 66)),
        ("click", NEW_PROJECT),
        ("wait", 500),
        ("type", name, 0),
        ("key", "Enter"),
        ("wait", 1000),
    ]


def describe(text, ms):
    """An instruction sent from the Describe tab, and time for the answer."""
    return [("click", DESCRIBE_FIELD), ("type", text, 0), ("key", "Enter"), ("move", REST), ("wait", ms)]


def mary_running():
    """The bench's program uploaded, run, and its input switched on."""
    return [("move", UPLOAD), ("wait", 300), ("upload", UPLOAD, "mary_had_a_little_lamb"), ("wait", 2000), ("click", RUN), ("wait", 500),
            ("click", MARY_SWITCH), ("move", REST), ("wait", 2000)]


# Nick's words in the 2026-11-25 demo.
LAMP_SPEC = "When the input goes high, a virtual lamp should turn on."
MOTOR_SPEC = "Now, not only should the lamp go high, the motor should spin at one revolution a second while it's active."


# A scene: query, layout, setup steps (off camera), take [(seconds, step...)], length.
# A step is ("click", pt) ("down", pt) ("up",) ("move", pt) ("key", combo)
# ("type", text, ms a key) ("wait", ms) ("upload", pt, file in uploads/) ("wheel", pt, dy)
SCENES = {
    # Test before you deploy: the bench's own program, uploaded and played with no machine.
    "mary": dict(
        query="?editor",
        layout=LAYOUT,
        setup=[("click", FILES_TAB)] + new_project("bench") + [("click", EXAMPLES)],
        take=[
            (0.8, "move", UPLOAD),
            (1.2, "upload", UPLOAD, "mary_had_a_little_lamb"),
            (3.0, "move", RUN),
            (3.4, "click", RUN),
            (4.4, "move", MARY_SWITCH),
            (5.0, "click", MARY_SWITCH),
            (5.6, "move", REST),
            (8.6, "move", WINDOW_KNOB),
            (9.0, "down", WINDOW_KNOB),
            (9.2, "move", (830, WINDOW_KNOB[1])),
            (9.6, "up"),
            (9.8, "move", REST),
        ],
        length=17.0,
    ),
    # Describe: Nick's own sentence from the 2026-11-25 demo, sent to the live free chat.
    "describe": dict(
        query="?editor",
        layout=LAYOUT,
        setup=[("click", FILES_TAB)] + new_project("lamp") + [("click", DESCRIBE_TAB)],
        take=[
            (0.6, "move", DESCRIBE_FIELD),
            (1.0, "click", DESCRIBE_FIELD),
            # Typing costs about 55 ms a key on top of the delay.
            (1.4, "type", LAMP_SPEC, 20),
            (6.0, "key", "Enter"),
            (6.4, "move", REST),
            # The answer took 15 to 40 s in probes; the films cut the wait.
            (42.0, "move", LAMP_INPUT),
            (42.6, "click", LAMP_INPUT),
            (43.2, "move", REST),
            (45.4, "move", LAMP_INPUT),
            (45.8, "click", LAMP_INPUT),
            (46.2, "move", REST),
            (47.6, "move", LAMP_INPUT),
            (48.0, "click", LAMP_INPUT),
            (48.4, "move", REST),
        ],
        length=52.0,
    ),
    # Every change, kept: Nick's second instruction from the demo, then back and forward through the versions.
    "history": dict(
        query="?editor",
        layout=LAYOUT,
        setup=[("click", FILES_TAB)] + new_project("lamp") + [("click", DESCRIBE_TAB)] + describe(LAMP_SPEC, 25000),
        take=[
            (0.6, "move", DESCRIBE_FIELD),
            (1.0, "click", DESCRIBE_FIELD),
            (1.4, "type", MOTOR_SPEC, 15),
            (7.0, "key", "Enter"),
            (7.4, "move", REST),
            (30.0, "move", BACK),
            (30.4, "click", BACK),
            (33.4, "click", FORWARD),
            (36.0, "move", VERSIONS),
            (36.4, "click", VERSIONS),
            (37.0, "move", REST),
        ],
        length=41.0,
    ),
    # Visualize: ask the diagram why the lamp is on, then zoom into the tune.
    "visualize": dict(
        query="?editor",
        layout=LAYOUT,
        setup=[("click", FILES_TAB)] + new_project("bench") + [("click", EXAMPLES)] + mary_running(),
        take=[
            (0.6, "move", LAMP_BLOCK),
            (1.0, "click", LAMP_BLOCK),
            (1.4, "move", (700, 120)),
            (4.0, "move", EMPTY_DIAGRAM),
            (4.4, "click", EMPTY_DIAGRAM),
            (5.0, "move", ZOOM_IN),
            (5.4, "click", ZOOM_IN),
            (6.2, "click", ZOOM_IN),
            (7.0, "move", REST),
        ],
        length=11.0,
    ),
    # One typo, one error: Nick's own slip from the demo (INT to IT), then Deploy refuses it.
    "code": dict(
        query="?editor",
        layout=LAYOUT,
        setup=[("click", FILES_TAB)] + new_project("bench") + [("click", EXAMPLES)] + mary_running()
        + [("click", CODE_TAB), ("down", PANE_EDGE), ("move", (560, PANE_EDGE[1])), ("up",), ("move", REST)],
        take=[
            (0.6, "move", INT_WORD),
            (1.0, "dbl", INT_WORD),
            (1.6, "type", "IT", 120),
            (2.2, "move", REST),
            (4.6, "key", "Shift+Meta+Enter"),
        ],
        length=9.0,
    ),
    # Plug in and go: scan the network, add what answers.
    "scan": dict(
        query="?editor",
        layout=LAYOUT,
        setup=[("click", FILES_TAB)] + new_project("cell") + [("click", DEVICES_TAB)],
        take=[
            (0.6, "move", SCAN),
            (1.0, "click", SCAN),
            (1.4, "move", REST),
            (3.4, "move", ADD_FOUND),
            (3.8, "click", ADD_FOUND),
            (4.6, "move", ADD_CONFIRM),
            (5.2, "click", ADD_CONFIRM),
            (5.6, "move", REST),
        ],
        length=9.0,
    ),
    # Nothing to install: the page from a cold load, the tank's proofs, one broken on purpose, then the editor.
    "front": dict(
        query="?front",
        layout=LAYOUT,
        cold=True,
        setup=[],
        # The page replays the writing of the program, then runs the proofs: the break comes after.
        take=[
            (0.5, "goto", "?front"),
            (17.5, "move", FAIL_HIGH),
            (18.0, "click", FAIL_HIGH),
            (18.5, "move", REST),
            (29.0, "move", OPEN_EDITOR),
            (29.5, "click", OPEN_EDITOR),
            (30.0, "move", REST),
        ],
        length=34.0,
    ),
}


def serve(root):
    class Handler(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *a, **k):
            super().__init__(*a, directory=root, **k)

        def log_message(self, *a):
            pass

    socketserver.TCPServer.allow_reuse_address = True
    srv = socketserver.TCPServer(("127.0.0.1", PORT), Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def act(page, step):
    kind = step[0]
    if kind == "click":
        page.mouse.click(*step[1])
    elif kind == "dbl":
        page.mouse.dblclick(*step[1])
    elif kind == "move":
        page.mouse.move(*step[1], steps=8)
    elif kind == "down":
        page.mouse.move(*step[1])
        page.mouse.down()
    elif kind == "up":
        page.mouse.up()
    elif kind == "key":
        page.keyboard.press(step[1])
    elif kind == "type":
        page.keyboard.type(step[1], delay=step[2])
    elif kind == "wait":
        page.wait_for_timeout(step[1])
    elif kind == "wheel":
        page.mouse.move(*step[1])
        page.mouse.wheel(0, step[2])
    elif kind == "goto":
        page.goto(f"http://127.0.0.1:{PORT}/playground/{step[1]}", wait_until="commit")
    elif kind == "upload":
        path = zipped(step[2])
        page.once("filechooser", lambda fc: fc.set_files(path))
        page.mouse.click(*step[1])
    else:
        raise ValueError(f"unknown step {step!r}")
    if kind in ("click", "dbl", "key"):
        page.wait_for_timeout(250)


def zipped(name):
    """uploads/<name>/ as a flat <name>.zip (files at its root), the way a
    downloaded project comes back: the playground names the project after it."""
    import zipfile

    src = os.path.join(UPLOADS, name)
    out = os.path.join(ZIPS, name + ".zip")
    os.makedirs(ZIPS, exist_ok=True)
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for f in sorted(os.listdir(src)):
            z.write(os.path.join(src, f), f)
    return out


def point(step):
    return step[1] if len(step) > 1 and isinstance(step[1], tuple) else None


def open_page(browser, scene):
    ctx = browser.new_context(
        viewport={"width": int(W * DPR), "height": int(H * DPR)}, device_scale_factor=DPR, color_scheme="dark"
    )
    page = ctx.new_page()
    page.on("pageerror", lambda e: print("  page error:", str(e)[:200]))
    page.goto(f"http://127.0.0.1:{PORT}/nothing-here.txt")
    page.evaluate("([k, v]) => localStorage.setItem(k, v)", [KEY, scene["layout"]])
    if scene.get("cold"):
        # The take loads the page itself (a "goto" step), on camera.
        page.goto("about:blank")
        return ctx, page
    page.goto(f"http://127.0.0.1:{PORT}/playground/{scene['query']}")
    page.wait_for_timeout(4000)
    page.mouse.move(*REST)
    for step in scene["setup"]:
        act(page, step)
    page.mouse.move(*REST)
    page.wait_for_timeout(1200)
    return ctx, page


def probe(browser, name, scene, work):
    out = os.path.join(work, name, "probe")
    shutil.rmtree(out, ignore_errors=True)
    os.makedirs(out)
    ctx, page = open_page(browser, scene)
    page.screenshot(path=os.path.join(out, "00-setup.png"))
    for i, (_, *step) in enumerate(scene["take"], 1):
        act(page, tuple(step))
        page.wait_for_timeout(600)
        page.screenshot(path=os.path.join(out, f"{i:02d}-{step[0]}.png"))
    page.wait_for_timeout(1500)
    page.screenshot(path=os.path.join(out, "99-end.png"))
    ctx.close()
    print(name, "probe shots in", out)


def record(browser, name, scene, work):
    base = os.path.join(work, name)
    frames = os.path.join(base, "frames")
    shutil.rmtree(base, ignore_errors=True)
    os.makedirs(frames)
    ctx, page = open_page(browser, scene)
    todo = list(scene["take"])
    events, stamps = [], []
    cdp = ctx.new_cdp_session(page)

    # Chromium's screencast pushes each frame it paints, with its own
    # wall-clock time, the clock the events are stamped on too.
    def on_frame(params):
        stamps.append(params["metadata"]["timestamp"])
        with open(os.path.join(frames, f"{len(stamps) - 1:05d}.jpg"), "wb") as f:
            f.write(base64.b64decode(params["data"]))
        cdp.send("Page.screencastFrameAck", {"sessionId": params["sessionId"]})

    cdp.on("Page.screencastFrame", on_frame)
    cdp.send(
        "Page.startScreencast",
        {"format": "jpeg", "quality": 92, "maxWidth": int(W * DPR * DPR), "maxHeight": int(H * DPR * DPR)},
    )
    t0 = time.monotonic()
    while time.monotonic() - t0 < scene["length"]:
        while todo and todo[0][0] <= time.monotonic() - t0:
            _, *step = todo.pop(0)
            at = time.time()
            act(page, tuple(step))
            pt = point(step)
            events.append({"t": at, "action": step[0], "x": pt[0] if pt else None, "y": pt[1] if pt else None,
                           "text": step[1] if step[0] in ("key", "type") else None, "end": time.time()})
        page.wait_for_timeout(10)
    stopped = time.time()  # a still page paints nothing: the last frame holds until the take ends
    cdp.send("Page.stopScreencast")
    ctx.close()
    end = max(stamps[-1] + 1 / 30, stopped)
    first = stamps[0]
    with open(os.path.join(base, "frames.txt"), "w") as f:
        for i, at in enumerate(stamps):
            nxt = stamps[i + 1] if i + 1 < len(stamps) else end
            f.write(f"file 'frames/{i:05d}.jpg'\nduration {max(nxt - at, 0.001):.4f}\n")
        f.write(f"file 'frames/{len(stamps) - 1:05d}.jpg'\n")
    for e in events:
        e["t"] = round(e["t"] - first, 3)
        e["end"] = round(e["end"] - first, 3)
    meta = {"scene": name, "frames": len(stamps), "seconds": round(end - first, 3), "points": [W, H], "events": events}
    with open(os.path.join(base, "events.json"), "w") as f:
        json.dump(meta, f, indent=1)
    app = os.path.join(FILM, "assets", "app")
    os.makedirs(app, exist_ok=True)
    subprocess.run(
        ["ffmpeg", "-v", "error", "-y", "-f", "concat", "-safe", "0", "-i", os.path.join(base, "frames.txt"),
         "-vf", "fps=30,scale=1920:1080:flags=lanczos", "-c:v", "libx264", "-crf", "14", "-pix_fmt", "yuv420p",
         "-an", os.path.join(app, f"{name}.mp4")],
        check=True,
    )
    shutil.copy(os.path.join(base, "events.json"), os.path.join(app, f"{name}.json"))
    print(name, len(stamps), "frames in", meta["seconds"], "s")
    for e in events:
        print("  ", e)


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    site, work = args[0], args[1]
    names = args[2:] or list(SCENES)
    probing = "--probe" in sys.argv
    srv = serve(site)
    try:
        with sync_playwright() as p:
            browser = p.chromium.launch(headless=True)
            for name in names:
                (probe if probing else record)(browser, name, SCENES[name], work)
            browser.close()
    finally:
        srv.shutdown()


if __name__ == "__main__":
    main()
