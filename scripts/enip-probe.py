"""Can this launch context reach an EtherNet/IP device? Sends one
ListIdentity (encapsulation command 0x63, empty body; docs/SOURCES.md, the
CIP Vol. 2 encapsulation chapter) to each target on UDP 44818 and prints who
answered within 1.5 s, or the error the send got.

Read-only on the network: one request per target, nothing else.

Which python runs it is the point (2026-11-07). macOS Local Network privacy
refuses LAN sends from non-Apple binaries started over SSH or from an agent
session, while Apple's /usr/bin/python3 is exempt, so:

    /opt/homebrew/bin/python3 -I scripts/enip-probe.py 192.168.3.60
        # says what a box started from here would get
    /usr/bin/python3 -I scripts/enip-probe.py 255.255.255.255 192.168.3.60
        # says whether the device answers at all

Exit status 0 when every target was answered, 1 otherwise.
"""

from __future__ import annotations

import socket
import struct
import sys
import time

PORT = 44818
LIST_IDENTITY = 0x63
WINDOW_S = 1.5


def request() -> bytes:
    # command, length, session handle, status, sender context, options
    return struct.pack("<HHII8sI", LIST_IDENTITY, 0, 0, 0, b"\0" * 8, 0)


def probe(target: str) -> tuple[list[str], str | None]:
    """The addresses that answered, and the send's error if it failed."""
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
        s.settimeout(0.3)
        try:
            s.sendto(request(), (target, PORT))
        except OSError as e:
            return [], str(e)
        seen: set[str] = set()
        end = time.monotonic() + WINDOW_S
        while time.monotonic() < end:
            try:
                data, addr = s.recvfrom(2048)
            except socket.timeout:
                continue
            if len(data) >= 2 and struct.unpack_from("<H", data)[0] == LIST_IDENTITY:
                seen.add(addr[0])
        return sorted(seen), None
    finally:
        s.close()


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__)
        return 2
    ok = True
    for target in argv:
        seen, err = probe(target)
        if err:
            print(f"{target}: send failed: {err}")
            ok = False
        elif seen:
            print(f"{target}: answered by {', '.join(seen)}")
        else:
            print(f"{target}: no answer in {WINDOW_S} s")
            ok = False
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
