"""The canvas smoke for a device's own configuration (Devices tab, the
Configuration section): a loopback box whose running map has a
`device:moxa-e1213` device, whose console is helmsman-devconf's simulator.

usage: python3 -I scripts/canvas-smoke-devconf.py [--dist DIR] [--out DIR]
       make canvas-smoke-devconf   (after make canvas-web and
                                    cargo build -p helmsman-plc)

It builds and starts the simulated console (examples/sim_console.rs) on
ephemeral loopback ports. The box reaches a console on port 80 and sees a
unit back on port 502 (both fixed in the box), and neither can be bound on
127.0.0.1 without root, so two forwarders listen on those ports (macOS lets
an unprivileged process bind them on the wildcard address; where that is
refused the script stops and says so) and pass each connection to the
simulator, or close it when the simulator has the port down. It then starts
a box (canvas-smoke.py's), makes a project with the map and a program that
reads the device, deploys it, saves a snapshot, stages a change (so the
section shows drift and "Staged, not applied yet"), and drives the page as
the engineer: the section, its drift lines, the Actions menu, the restart
confirmation, the 409 that names the module, and the password dialog. A
screenshot of each step is left in --out (default target/canvas-smoke) as
devconf-NN-*.png.
"""

import argparse
import importlib.util
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import urllib.request

from playwright.sync_api import sync_playwright

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
_spec = importlib.util.spec_from_file_location("smoke", os.path.join(ROOT, "scripts", "canvas-smoke.py"))
smoke = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(smoke)

PROJECT = "demo"
MAP = (
    "device moxa device:moxa-e1213 127.0.0.1:502\n"
    "in  moxa_di_00 bool moxa ch:DI-00\n"
    "out lamp bool box do:0 safe=false\n"
)
PROGRAM = (
    "PROGRAM reader\nVAR_INPUT moxa_di_00 : BOOL; END_VAR\nVAR_OUTPUT lamp : BOOL; END_VAR\n"
    "  lamp := moxa_di_00;\nEND_PROGRAM\n"
)


def forward(port, target):
    """Listens on every address at `port`; each connection goes to the
    simulator on `target`, or is closed at once while that is down."""
    srv = socket.socket()
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        srv.bind(("0.0.0.0", port))
    except OSError as e:
        sys.exit(f"cannot listen on port {port} ({e}): the box's console port is fixed; stop what holds it")
    srv.listen(16)

    def pipe(a, b):
        try:
            while data := a.recv(65536):
                b.sendall(data)
        except OSError:
            pass
        for s in (a, b):
            try:
                s.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass

    def serve():
        while True:
            try:
                c, _ = srv.accept()
            except OSError:
                return
            try:
                u = socket.create_connection(("127.0.0.1", target), timeout=2)
            except OSError:
                c.close()
                continue
            threading.Thread(target=pipe, args=(c, u), daemon=True).start()
            threading.Thread(target=pipe, args=(u, c), daemon=True).start()

    threading.Thread(target=serve, daemon=True).start()
    return srv


def start_sim():
    subprocess.run(
        ["cargo", "build", "-q", "-p", "helmsman-devconf", "--example", "sim_console"],
        cwd=ROOT, check=True,
    )
    p = subprocess.Popen(
        [os.path.join(ROOT, "target", "debug", "examples", "sim_console")],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
    )
    ports = dict(w.split("=") for w in p.stdout.readline().split())
    return p, int(ports["http"]), int(ports["modbus"])


class Api:
    """The box's API as the engineer, over a bearer session."""

    def __init__(self, base):
        self.base = base
        body = self.call("POST", "/api/login", {"user": smoke.USER, "password": smoke.PASSWORD}, auth=False)
        self.token = body["token"]

    def call(self, method, path, body=None, auth=True):
        h = {"Content-Type": "application/json"}
        if auth:
            h["Authorization"] = f"Bearer {self.token}"
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(self.base + path, data=data, method=method, headers=h)
        try:
            with urllib.request.urlopen(req, timeout=180) as r:
                text = r.read().decode()
        except urllib.error.HTTPError as e:
            sys.exit(f"{method} {path}: {e.code} {e.read().decode()[:300]}")
        return json.loads(text) if text else {}


def set_up(api):
    api.call("POST", "/api/projects", {"name": PROJECT})
    profile = open(os.path.join(ROOT, "docs", "devices", "moxa-iologik-e1213.device")).read()
    for name, text in (("main.st", PROGRAM), ("io.map", MAP), ("moxa-e1213.device", profile)):
        api.call("PUT", f"/api/projects/{PROJECT}/files/{name}", {"text": text, "version": None})
    # The policy wants a sim replay of an output-affecting change first.
    replay = api.call("POST", f"/api/projects/{PROJECT}/replay", {})["replay"]
    api.call("POST", f"/api/projects/{PROJECT}/deploy", {"replay": replay})
    api.call("POST", "/api/devices/moxa/config/snapshot", {})
    # A change staged on the unit: drift against the snapshot, and not applied.
    api.call("POST", "/api/devices/moxa/config/set", {"page": "13_1.htm", "changes": {"REST_ENABLE": "0"}})


def drive(ctx, base, out):
    # The 409 that asks the second question is the page working.
    smoke.EXPECTED = smoke.EXPECTED + ("status of 409",)
    r = smoke.Run(smoke.open_app(ctx, base, "", smoke.layout(pane="devices")), out, "devconf")
    r.wait(4000)

    def sign_in():
        r.click(1230, 100); r.wait(200)
        r.page.keyboard.type(smoke.USER)
        r.click(1230, 136); r.wait(200)
        r.page.keyboard.type(smoke.PASSWORD)
        r.key("Enter")
    r.step("signed-in", sign_in, smoke.HEADER_RIGHT, 6000)
    # The Devices pane is the remembered one: its key would shut it.
    r.wait(2500)
    r.shot("devices")
    r.click(92, 514); r.wait(700); r.shot("drift-lines")
    r.click(50, 214); r.wait(700); r.shot("actions-menu")
    r.click(120, 280); r.wait(900); r.shot("settings")
    r.key("Escape"); r.wait(500)
    r.click(50, 214); r.wait(600)
    r.click(110, 313); r.wait(900); r.shot("restart-confirm")
    # The box says a module reads the device: the question names it.
    r.click(49, 447); r.wait(2500); r.shot("restart-interrupts")
    # Go ahead: the unit restarts through the simulator (the box waits for it).
    r.click(84, 610); r.wait(800); r.shot("restarting")
    r.wait(9000); r.shot("restarted")
    r.click(50, 214); r.wait(600)
    r.click(156, 378); r.wait(900); r.shot("password")
    return r


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--dist", default=os.path.join(ROOT, "target", "canvas-dist"))
    ap.add_argument("--out", default=os.path.join(ROOT, "target", "canvas-smoke"))
    ap.add_argument("--interactive", default="", help="stop after the first screenshot (while mapping coordinates)")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    sim, http_port, modbus_port = start_sim()
    f80 = forward(80, http_port)
    f502 = forward(502, modbus_port)
    state = tempfile.mkdtemp(prefix="helmsman-smoke-devconf-")
    box_port = smoke.free_port()
    proc = smoke.start_box(a.dist, state, box_port)
    problems = []
    try:
        base = f"http://127.0.0.1:{box_port}"
        set_up(Api(base))
        with sync_playwright() as p:
            browser = p.chromium.launch()
            ctx = browser.new_context(
                viewport={"width": smoke.W * smoke.S, "height": smoke.H * smoke.S},
                device_scale_factor=smoke.S, color_scheme="dark",
            )
            r = drive(ctx, base, a.out)
            problems += r.problems()
            ctx.close()
            browser.close()
    finally:
        proc.terminate()
        try:
            proc.wait(5)
        except subprocess.TimeoutExpired:
            proc.kill()
        f80.close()
        f502.close()
        sim.stdin.close()
        sim.wait(5)
        shutil.rmtree(state, ignore_errors=True)
    for p_ in problems:
        print(p_)
    sys.exit(1 if problems else 0)


if __name__ == "__main__":
    main()
