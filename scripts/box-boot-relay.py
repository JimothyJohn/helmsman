#!/usr/bin/env python3
"""A scripted relay for the image boot test (scripts/box-boot-test.sh).

It speaks only what a box's first boot needs of the relay's protocol
("Pairing by code"): POST /v1/pair trades the one code for one token, once,
for one box id; GET /v1/health answers 200. Everything else is 404. It is
the contract's shape and nothing more: the real relay's limits, expiry and
storage are agent-relay's tests, not this script's.

    box-boot-relay.py PORT_FILE EVENTS_FILE CODE BOX_ID TOKEN

Binds 127.0.0.1 on a free port, writes the port to PORT_FILE, and appends
one line per event to EVENTS_FILE: `redeem ok`, or `redeem refused: why`.
Neither the code nor the token is ever written to a file or a log.
"""
import http.server
import json
import sys

port_file, events_file, code, box_id, token = sys.argv[1:6]
spent = False


def canon(c: str) -> str:
    c = "".join(ch for ch in c.upper() if ch not in "- \t")
    return c.translate(str.maketrans("OIL", "011"))


def event(text: str) -> None:
    with open(events_file, "a") as f:
        f.write(text + "\n")


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):  # no access log: it would carry paths only, but say nothing
        pass

    def reply(self, status: int, body: dict) -> None:
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path == "/v1/health":
            self.reply(200, {"ok": True, "protocol": 1})
        else:
            self.reply(404, {"error": {"type": "not_found", "message": "not found"}})

    def do_POST(self):
        global spent
        n = int(self.headers.get("Content-Length", "0"))
        raw = self.rfile.read(min(n, 4096))
        if self.path != "/v1/pair":
            self.reply(404, {"error": {"type": "not_found", "message": "not found"}})
            return
        refuse = {"error": {"type": "forbidden", "message": "pairing refused", "retryable": False}}
        try:
            body = json.loads(raw)
        except ValueError:
            event("redeem refused: not json")
            self.reply(400, {"error": {"type": "invalid_request", "message": "bad body"}})
            return
        why = None
        if self.headers.get("Authorization"):
            why = "a bearer header"
        elif set(body) != {"code", "box_id"}:
            why = "fields"
        elif body["box_id"] != box_id:
            why = "box id"
        elif canon(str(body["code"])) != canon(code):
            why = "code"
        elif spent:
            why = "spent"
        if why:
            event(f"redeem refused: {why}")
            self.reply(403, refuse)
            return
        spent = True
        event("redeem ok")
        self.reply(200, {"box_id": box_id, "token": token})


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(port_file, "w") as f:
    f.write(str(server.server_address[1]))
server.serve_forever()
