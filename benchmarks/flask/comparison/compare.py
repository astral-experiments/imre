"""Verified loopback comparison for the unchanged Flask benchmark."""
import argparse
import contextlib
import http.client
import json
import os
import platform
import random
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import zipfile
from pathlib import Path
from urllib.parse import quote, urlencode

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
import gate

JSON = "application/json"
HTML = "text/html; charset=utf-8"
WAITRESS = "waitress-3.0.2-py3-none-any.whl"
WAITRESS_HASH = "c56d67fd6e87c2ee598b76abdd4e96cfad1f24cacdea5078d382b1f9d7b5ed2e"


def cases():
    def row(route, path, body, media, values=None):
        return dict(route=route, path=path, body=body, media=media,
                    method="GET" if values is None else "POST",
                    status=200 if values is None else 201,
                    input="" if values is None else json.dumps({"values": values}))

    health = row("health", "/health", '{"ok":true}\n', JSON)
    names = [
        ("Ada <&>", [("suffix", " 'x'\"")], "Hello, Ada &lt;&amp;&gt; &#39;x&#39;&#34;"),
        ("Málaga + 東京", [], "Hello, Málaga + 東京!"),
        ("price+tax", [("suffix", " + "), ("suffix", "ignored")], "Hello, price+tax + "),
    ]
    totals = [
        [((i * 37) % 991) - 500 for i in range(128)],
        [2**95, 3, -2],
        [2**159 + 1, -(2**159), -(2**73 + 5), 7],
    ]
    items = []
    for (name, query, greeting), values in zip(names, totals, strict=True):
        path = "/hello/" + quote(name, safe="")
        if query:
            path += "?" + urlencode(query)
        body = json.dumps({"count": len(values), "total": sum(values)}, separators=(",", ":")) + "\n"
        items.extend([health, row("hello", path, greeting, HTML), row("total", "/total", body, JSON, values)])
    return items


@contextlib.contextmanager
def serve(command, core, *, capture_errors=True):
    with tempfile.TemporaryDirectory() as temp:
        ready = Path(temp) / "port"
        logfile = Path(temp) / "stderr" if capture_errors else Path(os.devnull)
        with logfile.open("w+") as log, subprocess.Popen(
            ["taskset", "-c", str(core), *command, ready], stdout=subprocess.DEVNULL, stderr=log
        ) as process:
            try:
                limit = time.monotonic() + 15
                while not ready.exists() or not (port := ready.read_text()).isdigit():
                    if process.poll() is not None or time.monotonic() > limit:
                        log.seek(0)
                        raise RuntimeError(f"{command[0]} did not start: {log.read()[-2000:]}")
                    time.sleep(0.01)
                yield int(port), process, log
            finally:
                process.terminate()
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()


def sessions(port):
    def visit(cookie=None):
        with contextlib.closing(http.client.HTTPConnection("127.0.0.1", port, timeout=5)) as conn:
            conn.request("GET", "/visits", headers={"Cookie": cookie} if cookie else {})
            response = conn.getresponse()
            assert response.status == 200 and response.getheader("Content-Type") == JSON
            new_cookie = response.getheader("Set-Cookie").partition(";")[0]
            assert new_cookie.startswith("session=") and "." in new_cookie
            return response.read().decode(), new_cookie

    first, cookie = visit()
    second, _ = visit(cookie)
    fresh, _ = visit()
    assert [first, second, fresh] == ['{"visits":1}\n', '{"visits":2}\n', '{"visits":1}\n']
    return [first, second, fresh]


def preflight(port, items, expected, flask):
    checked = []
    for case in items:
        response = gate.request(port, case["method"], case["path"], case["input"].encode() or None)
        assert (response["status"], response["content_type"], response["body"]) == (
            case["status"], case["media"], case["body"]), (case, response)
        checked.append(response["sha256"])
    for name, method, route, body in gate.CASES:
        value = gate.request(port, method, route, body)
        assert value == expected["loopback_tcp"][name], (name, value)
    with socket.create_connection(("127.0.0.1", port), timeout=5) as sock:
        sock.sendall(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        response = http.client.HTTPResponse(sock)
        response.begin()
        assert response.status == 200 and response.read() == b'{"ok":true}\n'
        assert sock.recv(1) == b"", "server did not close the TCP connection"
    return {"response_sha256": checked, "oracle_controls": [name for name, *_ in gate.CASES],
            "closed_connection": True, "response_http_version": response.version,
            "visits_fresh_replay_fresh": sessions(port) if flask else None}


def process_cpu(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rpartition(") ")[2].split()
    return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")


def core_cpu(core):
    fields = next(line for line in Path("/proc/stat").read_text().splitlines()
                  if line.startswith(f"cpu{core} ")).split()[1:9]
    ticks = list(map(int, fields))
    return (sum(ticks) - ticks[3] - ticks[4]) / os.sysconf("SC_CLK_TCK")


def executable(path):
    path = Path(path).resolve(strict=True)
    return {"path": str(path), "sha256": gate.digest(path)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("wheelhouse", "client", "output"):
        parser.add_argument(f"--{name}", required=True, type=Path)
    for name in ("node", "bun", "rust"):
        parser.add_argument(f"--{name}", type=Path)
    parser.add_argument("--compiler", type=Path)
    parser.add_argument("--runtime", type=Path)
    parser.add_argument("--wsgi-pair", action="store_true",
                        help="compare CPython/Werkzeug with CPython/Waitress using the extra pinned wheel")
    parser.add_argument("--hosted-launcher", type=Path,
                        help="compare CPython/Werkzeug with a Python-hosted LLVM launcher script (SITE, READY)")
    parser.add_argument("--hosted-artifact", type=Path, action="append", default=[],
                        help="hash a binary or source consumed by the separately built hosted launcher")
    parser.add_argument("--rounds", type=int, default=8, help="0 only checks correctness")
    parser.add_argument("--first-round", type=int, default=1, help="reproduce a specific round using its original order")
    parser.add_argument("--requests", type=int, default=2016)
    parser.add_argument("--warmup", type=int, default=252)
    parser.add_argument("--concurrency", type=int, default=8)
    parser.add_argument("--mixed-only", action="store_true",
                        help="run the equal-weight mix at the selected concurrency only")
    parser.add_argument("--server-cpu", type=int, default=28)
    parser.add_argument("--client-cpus", default="29,30,31")
    args = parser.parse_args()
    expected = json.loads((HERE.parent / "expected.json").read_text())
    if sys.implementation.name != "cpython" or sys.version.split()[0] != expected["python"]:
        parser.error(f"run with CPython {expected['python']}")
    if bool(args.compiler) != bool(args.runtime):
        parser.error("supply both --compiler and --runtime")
    paired = args.wsgi_pair or args.hosted_launcher
    if not paired and not all((args.node, args.bun, args.rust)):
        parser.error("the cross-language comparison requires --node, --bun and --rust")
    if args.first_round < 1 or args.rounds < 0:
        parser.error("first round must be positive and rounds nonnegative")
    if any(n < args.concurrency or n % 9 for n in (args.warmup, args.requests)):
        parser.error("requests and warmup must each be a multiple of nine and cover all clients")
    if args.output.exists():
        parser.error(f"output already exists: {args.output}")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    items = cases()
    sources = [HERE.parent / f for f in ("tiny_flask_app.py", "serve.py", "gate.py", "requirements.txt", "expected.json")]
    sources.extend(HERE / f for f in ("compare.py", "control.mjs", "client.go", "rust/Cargo.toml", "rust/Cargo.lock", "rust/src/main.rs"))
    if args.wsgi_pair:
        sources.append(HERE / "serve_waitress.py")
    inventory = {name: executable(path) for name, path in
                 {"cpython": sys.executable, "node": args.node, "bun": args.bun,
                  "rust": args.rust, "client": args.client}.items() if path}
    versions = {name: subprocess.check_output([inventory[name]["path"], "--version"], text=True).strip()
                for name in ("cpython", "node", "bun") if name in inventory}
    host = {"kernel": platform.release(), "machine": platform.machine(),
            "cpu": next(line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines()
                        if line.startswith("model name"))}
    report = {"base": subprocess.check_output(["git", "-C", HERE, "rev-parse", "HEAD"], text=True).strip(),
              "sources": {str(path.relative_to(HERE.parent)): gate.digest(path) for path in sources},
              "host": host, "versions": versions, "executables": inventory,
              "settings": {k: getattr(args, k) for k in ("first_round", "rounds", "requests", "warmup", "concurrency", "server_cpu", "client_cpus", "mixed_only")},
              "cases": items, "preflight": {}, "trials": []}
    assert report["sources"]["tiny_flask_app.py"] == expected["app_sha256"]

    def save():
        tmp = args.output.with_suffix(".partial")
        tmp.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        tmp.replace(args.output)

    with tempfile.TemporaryDirectory() as temp:
        work = Path(temp)
        site = work / "site"
        site.mkdir()
        report["wheel_sha256"] = gate.stage(args.wheelhouse, site)
        speedup = subprocess.check_output([sys.executable, "-ESB", "-c",
                    "from markupsafe import _speedups; print(_speedups.__file__)"], cwd=site, text=True).strip()
        assert Path(speedup) == site / "markupsafe" / expected["speedups"]
        report["markupsafe_extension"] = expected["speedups"]
        commands = {"cpython": [sys.executable, "-ESB", site / "serve.py"]}
        if not paired:
            commands.update(node=[args.node.resolve(), HERE / "control.mjs"],
                            bun=[args.bun.resolve(), HERE / "control.mjs"], rust=[args.rust.resolve()])
        if args.wsgi_pair:
            wheel = args.wheelhouse / WAITRESS
            if gate.digest(wheel) != WAITRESS_HASH:
                raise ValueError(f"incorrect {WAITRESS}")
            with zipfile.ZipFile(wheel) as archive:
                archive.extractall(site)
            report["wheel_sha256"][WAITRESS] = WAITRESS_HASH
            shutil.copyfile(HERE / "serve_waitress.py", site / "serve_waitress.py")
            commands["waitress"] = [sys.executable, "-ESB", site / "serve_waitress.py"]
        if args.hosted_launcher:
            inventory["hosted_llvm"] = executable(args.hosted_launcher)
            report["hosted_artifacts"] = [executable(path) for path in args.hosted_artifact]
            commands["hosted_llvm"] = [sys.executable, "-ISB", args.hosted_launcher.resolve(), site]
        fixtures = work / "fixtures.json"
        fixtures.write_text(json.dumps(items))
        if args.compiler:
            inventory.update(compiler=executable(args.compiler), runtime=executable(args.runtime))
            binary = work / "imre-server"
            result = subprocess.run([args.compiler.resolve(), "build", site / "serve.py",
                "--module-root", site, "--runtime", args.runtime.resolve(), "--opt-level", "3",
                "-o", binary], text=True, capture_output=True, timeout=120)
            report["imre_build"] = {"status": result.returncode, "stdout": result.stdout, "stderr": result.stderr}
            if result.returncode == 0:
                commands["imre"] = [binary]
                report["imre_build"]["binary_sha256"] = gate.digest(binary)
        else:
            report["imre_build"] = {"status": None, "reason": "compiler and runtime not supplied"}
        for name, command in list(commands.items()):
            try:
                with serve(command, args.server_cpu) as (port, _, log):
                    flask = name in ("cpython", "imre", "waitress", "hosted_llvm")
                    checks = preflight(port, items, expected, flask)
                    if flask:
                        cls, (key,) = expected["view_exception"]
                        log.seek(0)
                        line = f"{cls}: {key!r}"
                        assert log.read().splitlines().count(line) == 1, f"missing traceback: {line}"
                        checks["view_exception"] = line
                    report["preflight"][name] = checks
            except (OSError, RuntimeError, ValueError, AssertionError, http.client.HTTPException) as exc:
                report["preflight"][name] = {"error": repr(exc)}
                save()
                if name != "imre":
                    raise
                del commands[name]
        save()
        print("Preflight passed:", ", ".join(commands), flush=True)
        rng = random.Random(431)
        order = list(commands)
        rng.shuffle(order)
        profiles = [("mixed", 1), ("mixed", args.concurrency)]
        profiles.extend((name, args.concurrency) for name in ("health", "hello", "total"))
        for r in range(args.first_round - 1 + args.rounds):
            offset = r % len(order)
            baseline = order if (r // len(order)) % 2 == 0 else list(reversed(order))
            for name in baseline[offset:] + baseline[:offset]:
                experiments = [("mixed", args.concurrency)] if args.mixed_only else list(profiles)
                if not args.mixed_only and name in ("cpython", "imre", "waitress", "hosted_llvm"):
                    experiments.extend([("session-new", 1), ("session-replay", 1)])
                rng.shuffle(experiments)
                if r < args.first_round - 1:
                    continue
                with serve(commands[name], args.server_cpu, capture_errors=False) as (port, process, _):
                    for profile, c in experiments:
                        call = ["taskset", "-c", args.client_cpus, args.client.resolve(),
                                "-addr", f"127.0.0.1:{port}", "-cases", fixtures,
                                "-profile", profile, "-c", str(c)]
                        env = {**os.environ, "GOMAXPROCS": str(len(args.client_cpus.split(",")))}
                        subprocess.run([*call, "-n", str(args.warmup)], check=True, stdout=subprocess.DEVNULL, env=env)
                        client_cores = [int(cpu) for cpu in args.client_cpus.split(",")]
                        server0, busy0 = process_cpu(process.pid), core_cpu(args.server_cpu)
                        client_busy0 = sum(core_cpu(cpu) for cpu in client_cores)
                        started_at = time.time()
                        start = time.monotonic()
                        sample = json.loads(subprocess.check_output([*call, "-n", str(args.requests)], env=env))
                        sample["wall_seconds"] = time.monotonic() - start
                        server = process_cpu(process.pid) - server0
                        sample.update(round=r + 1, server=name, profile=profile, concurrency=c,
                                      server_cpu_seconds=server, started_at=started_at, loadavg=os.getloadavg(),
                                      other_client_core_seconds=max(0, sum(core_cpu(cpu) for cpu in client_cores)
                                          - client_busy0 - sample["ClientCPUSeconds"]),
                                      other_server_core_seconds=max(0, core_cpu(args.server_cpu) - busy0 - server))
                        report["trials"].append(sample)
                print(f"Round {r + 1}: {name}", flush=True)
                save()


if __name__ == "__main__":
    main()
