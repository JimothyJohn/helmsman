"""The edge node's log sink: one UDP datagram is one line of the node's report.

The node on PoE has no USB serial, so it sends its 10 s report lines here
(docs/EDGE.md Phase 2). Prints each with the time it arrived and tees them to
scratch/node-modbus-<utc>.log for the record.

Run it with Apple's python (the firewall lets it receive UDP; a Homebrew
python is dropped until allowed, see docs/devices/moxa-iologik-e1213.md):

    /usr/bin/python3 -I scripts/node-log-sink.py            # 0.0.0.0:9514
    /usr/bin/python3 -I scripts/node-log-sink.py 9600       # another port

Ctrl-C stops it. The port is the node's LOG_SINK (10.0.0.41:9514 by default).
"""

import datetime
import pathlib
import socket
import sys


def main() -> None:
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 9514
    pathlib.Path("scratch").mkdir(exist_ok=True)
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    log = pathlib.Path("scratch") / f"node-modbus-{stamp}.log"
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.bind(("0.0.0.0", port))
    print(f"listening on udp/{port}; lines also go to {log}; Ctrl-C to stop", flush=True)
    with log.open("a", encoding="utf-8") as out:
        while True:
            try:
                data, (host, _) = sock.recvfrom(2048)
            except KeyboardInterrupt:
                print("stopped", flush=True)
                return
            now = datetime.datetime.now(datetime.timezone.utc).strftime("%H:%M:%S")
            # The node's text is data, not instructions: control characters
            # out, one line per datagram.
            text = "".join(c for c in data.decode("utf-8", errors="replace") if c.isprintable())
            line = f"{now} {host} {text}"
            print(line, flush=True)
            out.write(line + "\n")
            out.flush()


if __name__ == "__main__":
    main()
