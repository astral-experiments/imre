"""Exercise fragmented reads, partial writes and independent benchmark clients."""

import argparse
import json
from pathlib import Path
import socket
import struct
import sys
import time
sys.path.insert(0, str(Path(__file__).resolve().parent))
from run import HERE, serving

GET = b"GET /hello HTTP/1.1\r\nHost: localhost\r\n\r\n"
CLOSE = GET[:-2] + b"Connection: close\r\n\r\n"


def read(f):
    line = f.readline()
    assert line.startswith(b"HTTP/1.1 "), line
    status = int(line.split()[1])
    headers = {}
    while True:
        line = f.readline()
        if line == b"\r\n":
            break
        assert line, headers
        k, v = line.split(b":", 1)
        headers[k.lower()] = v.strip()
    n = int(headers[b"content-length"])
    body = f.read(n)
    assert len(body) == n
    return status, body


def validate(cmd):
    req = b"GET /hello HTTP/1.1\r\nHost: localhost\r\n\r\n"
    close = b"GET /missing HTTP/1.1\r\nHost: localhost\r\nCoNnEcTiOn: close\r\n\r\n"
    checked = []
    with serving(cmd, None) as (_, port):
        with (
            socket.create_connection(("127.0.0.1", port), 4) as s,
            s.makefile("rb") as f,
        ):
            for part in (req[:13], req[13:-1], req[-1:]):
                s.sendall(part)
                time.sleep(0.005)
            assert read(f) == (200, b"hello\n")
            checked.append("fragmented header")
            s.sendall(req + close)
            assert (
                read(f) == (200, b"hello\n")
                and read(f) == (404, b"not found\n")
                and f.read(1) == b""
            )
            checked.extend(
                ["pipelining", "keep-alive", "404", "case-insensitive close", "EOF"]
            )
        for raw, status, label in (
            (b"POST /hello HTTP/1.1\r\nHost: local\r\n\r\n", 400, "bad request"),
            (b"GET /hello HTTP/1.1\r\nX: " + b"a" * 8200, 431, "header cap"),
        ):
            with (
                socket.create_connection(("127.0.0.1", port), 4) as s,
                s.makefile("rb") as f,
            ):
                s.sendall(raw)
                assert read(f) == (status, b"")
                assert f.read(1) == b""
                checked.append(label)
    return checked


def check(command):
    with serving(command, None) as (_, port):
        address = ("127.0.0.1", port)
        with socket.create_connection(address, 3) as first:
            first.sendall(GET)
            with first.makefile("rb") as f:
                assert read(f) == (200, b"hello\n")
            with socket.create_connection(address, 3) as second:
                second.sendall(CLOSE)
                second.settimeout(0.5)
                start = time.monotonic()
                try:
                    before = bool(second.recv(1, socket.MSG_PEEK))
                except TimeoutError:
                    before = False
                elapsed_ms = 1000 * (time.monotonic() - start)
                first.close()
                second.settimeout(4)
                with second.makefile("rb") as f:
                    assert read(f) == (200, b"hello\n")
                return {
                    "served_while_first_idle": before,
                    "wait_ms": round(elapsed_ms, 2),
                    "body_verified": True,
                }


def pressure(cmd):
    with serving([*cmd, "2048"], None) as (_, port):
        address = ("127.0.0.1", port)
        with socket.socket() as slow:
            slow.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 2048)
            slow.settimeout(8)
            slow.connect(address)
            # The server also uses a 2 KiB send buffer for this control only.
            n = 1000
            slow.sendall(GET * n + CLOSE)
            time.sleep(0.2)
            with socket.create_connection(address, 2) as fast:
                fast.sendall(CLOSE)
                fast.settimeout(0.5)
                start = time.monotonic()
                try:
                    seen = bool(fast.recv(1, socket.MSG_PEEK))
                except TimeoutError:
                    seen = False
                delay = round((time.monotonic() - start) * 1000, 2)
                assert seen, f"slow reader blocked another client for {delay} ms"
                with fast.makefile("rb") as r:
                    assert read(r) == (200, b"hello\n") and not r.read(1)
            # Resume reading; all queued replies are delivered in order, then clean EOF.
            slow.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 262144)
            with slow.makefile("rb") as r:
                for _ in range(n + 1):
                    assert read(r) == (200, b"hello\n")
                assert not r.read(1)
        reset = socket.create_connection(address, 2)
        reset.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
        reset.sendall(GET * 100)
        reset.close()
        with socket.create_connection(address, 2) as fast, fast.makefile("rb") as r:
            fast.sendall(CLOSE)
            assert read(r) == (200, b"hello\n") and not r.read(1)
        # A half-written header on one connection must not delay another.
        with socket.create_connection(address, 2) as first, first.makefile("rb") as a:
            first.sendall(CLOSE[:9])
            with (
                socket.create_connection(address, 2) as second,
                second.makefile("rb") as b,
            ):
                second.sendall(CLOSE)
                assert read(b) == (200, b"hello\n")
            first.sendall(CLOSE[9:])
            assert read(a) == (200, b"hello\n")
        return {
            "verified_queued_responses": n + 1,
            "other_client_wait_ms": delay,
            "client_reset_survived": True,
            "fragmented_client_independent": True,
        }


def verify(command):
    wire = validate(command)
    idle = check(command)
    assert idle["served_while_first_idle"], (
        "idle keep-alive prevented independent progress"
    )
    return {"wire": wire, "idle": idle, "slow_reader": pressure(command)}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "binary", type=Path, nargs="?", help="Compiled serve.py executable"
    )
    parser.add_argument(
        "--python", help="Interpreter for the identical serve.py source"
    )
    args = parser.parse_args()
    if args.binary and args.python:
        parser.error("choose the compiled binary or --python")
    command = (
        [args.binary.resolve()]
        if args.binary
        else [args.python or sys.executable, "-IB", HERE / "serve.py"]
    )
    print(json.dumps(verify(command), indent=2))
