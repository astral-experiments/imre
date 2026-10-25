"""Exercise the task board through real HTTP, including restart and invalid framing."""

import argparse
from contextlib import contextmanager
import http.client
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import time

HERE = Path(__file__).resolve().parent


@contextmanager
def server(cmd, journal):
    with socket.socket() as scout:
        scout.bind(("127.0.0.1", 0))
        port = scout.getsockname()[1]
    with tempfile.TemporaryFile() as log:
        proc = subprocess.Popen([*cmd, str(port), str(journal)], stdout=log, stderr=log)
        try:
            for _ in range(200):
                if proc.poll() is not None:
                    log.seek(0)
                    raise RuntimeError(log.read().decode(errors="replace"))
                try:
                    with socket.create_connection(("127.0.0.1", port), 0.1):
                        pass
                    break
                except OSError:
                    time.sleep(0.02)
            else:
                raise RuntimeError("server did not start")
            yield port
        finally:
            proc.terminate()
            try:
                proc.wait(3)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()


def api(port, method, path, body=None, media="application/json"):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=3)
    try:
        conn.request(method, path, body=body, headers={"Content-Type": media})
        r = conn.getresponse()
        data = r.read()
        return r.status, dict(r.getheaders()), data
    finally:
        conn.close()


def expect(port, method, path, status, body=None, media="application/json"):
    code, headers, data = api(port, method, path, body, media)
    assert code == status, (method, path, code, data)
    assert headers["Content-Type"] == "application/json" and int(
        headers["Content-Length"]
    ) == len(data)
    return json.loads(data)


def raw_reply(stream):
    line = stream.readline()
    assert line.startswith(b"HTTP/1.1 "), line
    code = int(line.split()[1])
    headers = {}
    while (line := stream.readline()) != b"\r\n":
        assert line, "EOF in headers"
        k, v = line.split(b":", 1)
        headers[k.lower()] = v.strip()
    size = int(headers[b"content-length"])
    data = stream.read(size)
    assert len(data) == size
    return code, data


def run(cmd):
    checks = []
    with tempfile.TemporaryDirectory() as td:
        journal = Path(td) / "tasks.jsonl"
        title = 'Ship π 🍎 <script> "quoted"'
        body = json.dumps({"title": "  " + title + "  "}, ensure_ascii=False).encode()
        expected = {"id": 1, "title": title, "done": False}
        with server(cmd, journal) as port:
            c, headers, index = api(port, "GET", "/")
            assert (
                c == 200
                and b"<form" in index
                and b"textContent" in index
                and headers["Content-Type"].startswith("text/html")
            )
            assert expect(port, "GET", "/tasks", 200) == []
            assert expect(port, "POST", "/tasks", 201, body) == expected
            assert expect(port, "GET", "/tasks/1?view=full", 200) == expected
            c, headers, data = api(port, "HEAD", "/tasks/1")
            assert c == 200 and not data and int(headers["Content-Length"]) > 0
            assert expect(port, "GET", "/tasks", 200) == [expected]
            c, headers, data = api(port, "PUT", "/tasks")
            assert c == 405 and set(headers["Allow"].split(", ")) == {
                "GET",
                "HEAD",
                "POST",
            }
            expect(port, "GET", "/tasks/nope", 404)
            expect(port, "GET", "/tasks/88", 404)
            for invalid in (
                b"{",
                b"{}",
                b"[]",
                b'{"title":1}',
                b'{"title":true}',
                b'{"title":null}',
                b'{"title":"  "}',
                b"\xff",
                json.dumps({"title": "x" * 201}).encode(),
            ):
                expect(port, "POST", "/tasks", 400, invalid)
            expect(port, "POST", "/tasks", 415, body, "text/plain")
            assert expect(port, "GET", "/tasks", 200) == [expected]
            checks.extend(
                [
                    "browser page",
                    "route id and query",
                    "Unicode JSON create/read/list",
                    "HEAD and Allow",
                    "400/404/405/415",
                ]
            )
            expected["done"] = True
            assert expect(port, "POST", "/tasks/1/done", 200) == expected
            assert expect(port, "POST", "/tasks/1/done", 200) == expected
            checks.append("idempotent completion")
            payload = b'{"title":"second"}'
            head = (
                b"POST /tasks HTTP/1.1\r\nHost: local\r\nContent-Type: application/json\r\nContent-Length: "
                + str(len(payload)).encode()
                + b"\r\n\r\n"
            )
            with (
                socket.create_connection(("127.0.0.1", port), 3) as s,
                s.makefile("rb") as stream,
            ):
                s.sendall(head[:12])
                s.sendall(head[12:] + payload[:3])
                assert expect(port, "GET", "/tasks/1", 200) == expected
                s.sendall(
                    payload[3:]
                    + b"GET /tasks/2 HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n"
                )
                c, d = raw_reply(stream)
                assert c == 201 and json.loads(d)["title"] == "second"
                c, d = raw_reply(stream)
                assert c == 200 and json.loads(d)["id"] == 2 and stream.read(1) == b""
            checks.append("fragmented body, independent client and pipelined GET")
            for extra, code in [
                (b"Content-Length: 2\r\nContent-Length: 3", 400),
                (b"Transfer-Encoding: chunked", 400),
                (b"Content-Length: -1", 400),
                (b"Content-Length: 16385", 413),
                (b"Expect: 100-continue", 417),
                (b"X: " + b"x" * 8200, 431),
            ]:
                with (
                    socket.create_connection(("127.0.0.1", port), 3) as s,
                    s.makefile("rb") as stream,
                ):
                    s.sendall(
                        b"POST /tasks HTTP/1.1\r\nHost: local\r\n" + extra + b"\r\n\r\n"
                    )
                    assert raw_reply(stream)[0] == code and not stream.read(1), (
                        extra[:30],
                        code,
                    )
            checks.append("reject ambiguous framing and header/body limits")
        assert [json.loads(s) for s in journal.read_text().splitlines()] == [
            ["add", title],
            ["done", "1"],
            ["add", "second"],
        ]
        with server(cmd, journal) as port:
            assert expect(port, "GET", "/tasks", 200) == [
                expected,
                {"id": 2, "title": "second", "done": False},
            ]
            assert expect(port, "POST", "/tasks", 201, b'{"title":"third"}')["id"] == 3
        checks.append("journal restart and next ID")
    return checks


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    group = p.add_mutually_exclusive_group(required=True)
    group.add_argument("--python")
    group.add_argument("--binary", type=Path)
    args = p.parse_args()
    command = (
        [args.binary.resolve()]
        if args.binary
        else [args.python, "-EB", HERE / "app.py"]
    )
    print(json.dumps(run(command), indent=2))
