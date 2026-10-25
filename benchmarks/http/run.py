"""Run the same verified HTTP/1.1 workload against public Imre and reference servers."""

import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import random
import socket
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def sha(path):
    with path.open("rb") as src:
        return hashlib.file_digest(src, "sha256").hexdigest()


def release(cargo, manifest, packages, target_names):
    cmd = [
        *cargo,
        "build",
        "--manifest-path",
        str(manifest),
        "--locked",
        "--release",
        "--message-format=json",
    ]
    for pkg in packages:
        cmd.extend(("-p", pkg))
    outputs = {}
    with subprocess.Popen(cmd, stdout=subprocess.PIPE, text=True) as build:
        for line in build.stdout:
            msg = json.loads(line)
            if msg.get("reason") == "compiler-message":
                print(msg["message"].get("rendered", ""), file=sys.stderr, end="")
            if msg.get("reason") != "compiler-artifact":
                continue
            name = msg["target"]["name"]
            if name not in target_names:
                continue
            if msg["profile"]["opt_level"] != "3":
                raise RuntimeError(
                    f"{name} was compiled at opt-level {msg['profile']['opt_level']}"
                )
            path = msg.get("executable") or next(
                (p for p in msg["filenames"] if p.endswith(".a")), None
            )
            if path:
                outputs[name] = Path(path)
        if build.wait() != 0:
            raise RuntimeError("Cargo release build failed")
    if set(outputs) != set(target_names):
        raise RuntimeError(
            f"Missing release artifacts: {set(target_names) - set(outputs)}"
        )
    return outputs


@contextlib.contextmanager
def serving(command, core):
    with socket.socket() as scout:
        scout.bind(("127.0.0.1", 0))
        port = scout.getsockname()[1]
    prefix = [] if core is None else ["taskset", "-c", str(core)]
    with tempfile.TemporaryFile() as log:
        proc = subprocess.Popen(
            [*prefix, *map(str, command), str(port)], stdout=log, stderr=log
        )
        try:
            deadline = time.monotonic() + 10
            while True:
                if proc.poll() is not None:
                    log.seek(0)
                    raise RuntimeError(log.read().decode(errors="replace"))
                try:
                    with socket.create_connection(("127.0.0.1", port), 0.1):
                        pass
                    break
                except OSError:
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(0.02)
            yield proc, port
        finally:
            proc.terminate()
            try:
                proc.wait(3)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--servers", default="imre,cpython,selectors,node,bun,rust")
    ap.add_argument("--python", default=sys.executable)
    ap.add_argument("--node", default="node")
    ap.add_argument("--bun", default="bun")
    ap.add_argument("--clang", default="clang")
    ap.add_argument("--toolchain", help="Optional Rust toolchain, such as ohm")
    ap.add_argument("--clients", default="16,64")
    ap.add_argument(
        "--rate",
        type=int,
        default=0,
        help="Fixed aggregate arrivals per second (default: saturation)",
    )
    ap.add_argument("--header-bytes", type=int, default=0)
    ap.add_argument("--rounds", type=int, default=5)
    ap.add_argument("--seconds", type=float, default=2.5)
    ap.add_argument("--warmup", type=float, default=1)
    ap.add_argument("--server-core", type=int)
    ap.add_argument(
        "--client-cores", help="Comma-separated CPU numbers for the load generator"
    )
    ap.add_argument("--out", type=Path, required=True)
    a = ap.parse_args()
    selected = a.servers.split(",")
    counts = [int(c) for c in a.clients.split(",")]
    if len(set(selected)) != len(selected) or set(selected) - {
        "imre",
        "cpython",
        "selectors",
        "node",
        "bun",
        "rust",
    }:
        ap.error("choose each of imre,cpython,selectors,node,bun,rust at most once")
    if (
        not counts
        or min(counts) < 1
        or a.rounds < 1
        or a.seconds <= 0
        or a.warmup <= 0
        or a.header_bytes < 0
        or a.rate < 0
        or a.rate
        and min(a.warmup, a.seconds) * a.rate < max(counts)
    ):
        ap.error(
            "positive trials, duration and clients, and nonnegative header length are required"
        )
    if bool(a.client_cores) != (a.server_core is not None):
        ap.error("supply both --server-core and --client-cores or neither")
    if a.client_cores and a.server_core in {int(c) for c in a.client_cores.split(",")}:
        ap.error("use distinct load-generator and server cores")
    if a.out.exists():
        ap.error("choose a new output file to preserve earlier trials")
    cargo = ["cargo"] + (["+" + a.toolchain] if a.toolchain else [])
    record = {
        "git": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip(),
        "python_source_sha256": sha(HERE / "serve.py"),
        "settings": vars(a) | {"out": str(a.out)},
        "runtime_versions": {},
        "release_artifacts": {},
        "observations": [],
        "failed_trials": [],
    }
    with tempfile.TemporaryDirectory(prefix="imre-http-") as td:
        tmp = Path(td)
        client = tmp / "loadgen"
        subprocess.run(["go", "build", "-o", client, HERE / "client.go"], check=True)
        record["runtime_versions"]["go"] = subprocess.check_output(
            ["go", "version"], text=True
        ).strip()
        servers = {
            "cpython": [a.python, "-IB", HERE / "serve.py"],
            "selectors": [a.python, "-IB", HERE / "cpython_selectors.py"],
            "node": [a.node, HERE / "node.cjs"],
            "bun": [a.bun, HERE / "bun.js"],
        }
        if "imre" in selected:
            artifacts = release(
                cargo,
                ROOT / "Cargo.toml",
                ["imre-compiler", "imre-runtime"],
                ["imre", "imre_runtime"],
            )
            record["release_artifacts"] = {
                name: {"sha256": sha(p), "opt_level": "3"}
                for name, p in artifacts.items()
            }
            exe = tmp / "imre-http"
            subprocess.run(
                [
                    artifacts["imre"],
                    "build",
                    HERE / "serve.py",
                    "--opt-level",
                    "3",
                    "--no-cache",
                    "--runtime",
                    artifacts["imre_runtime"],
                    "--clang",
                    a.clang,
                    "-o",
                    exe,
                ],
                check=True,
            )
            record["imre_executable_sha256"] = sha(exe)
            servers["imre"] = [exe]
        if "rust" in selected:
            name = "imre-http-hyper-control"
            artifacts = release(cargo, HERE / "rust/Cargo.toml", [name], [name])
            servers["rust"] = [artifacts[name]]
            record["runtime_versions"]["rust_binary_sha256"] = sha(artifacts[name])
        for name in set(selected) & {"cpython", "selectors", "node", "bun"}:
            record["runtime_versions"][name] = subprocess.check_output(
                [servers[name][0], "--version"], text=True
            ).strip()

        def load(port, c, seconds):
            cmd = [] if not a.client_cores else ["taskset", "-c", a.client_cores]
            cmd.extend(
                [
                    str(client),
                    "-addr",
                    f"127.0.0.1:{port}",
                    "-c",
                    str(c),
                    "-duration",
                    f"{seconds}s",
                    "-rate",
                    str(a.rate),
                    "-request-padding",
                    str(a.header_bytes),
                ]
            )
            env = dict(os.environ)
            env["GOMAXPROCS"] = (
                str(len(a.client_cores.split(","))) if a.client_cores else "2"
            )
            r = subprocess.run(
                cmd, capture_output=True, text=True, timeout=seconds + 12, env=env
            )
            if r.returncode:
                raise RuntimeError(r.stderr.strip())
            row = json.loads(r.stdout)
            w = row["worker_requests"]
            if row["errors"] or len(w) != c or min(w) < 1 or sum(w) != row["requests"]:
                raise RuntimeError("client validation failed")
            if a.rate and row["scheduled_requests"] != row["requests"]:
                raise RuntimeError("scheduled requests missing")
            return row

        order = list(selected)
        random.Random(74323).shuffle(order)
        for i in range(a.rounds):
            for c in counts if i % 2 == 0 else reversed(counts):
                for name in order[i % len(order) :] + order[: i % len(order)]:
                    key = {"server": name, "clients": c, "round": i + 1}
                    with serving(servers[name], a.server_core) as (_, port):
                        phase = "warmup"
                        try:
                            load(port, c, a.warmup)
                            phase = "timed"
                            row = load(port, c, a.seconds)
                        except (RuntimeError, subprocess.TimeoutExpired) as e:
                            record["failed_trials"].append(
                                key | {"phase": phase, "error": str(e)}
                            )
                        else:
                            record["observations"].append(key | row)
                            print(
                                f"{name} {c} clients: {row['rps']:,.0f} req/s, p95 {row['p95_us'] / 1000:.2f} ms",
                                flush=True,
                            )
                    stage = a.out.with_suffix(".partial")
                    stage.write_text(json.dumps(record, indent=2) + "\n")
                    stage.replace(a.out)
    if record["failed_trials"]:
        sys.exit("Some trials failed; see the recorded errors")


if __name__ == "__main__":
    main()
