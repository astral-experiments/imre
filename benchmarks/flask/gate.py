"""Compare a Flask application over real TCP in CPython and Imre."""
import argparse
import hashlib
import http.client
import json
import os
import signal
import shutil
import subprocess
import sys
import tempfile
import time
import zipfile
from http.cookies import SimpleCookie
from pathlib import Path
from urllib.parse import quote, urlencode, urlsplit

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import native_identity

HELLO_PATH = "/hello/" + quote("Málaga <&> 東京") + "?" + urlencode([("suffix", " 'x'\""), ("suffix", "unused")])
CASES = (
    ("health", "GET", "/health", None),
    ("bigint", "POST", "/total", json.dumps({"values": [2**95, 3, -2]}).encode()),
    ("missing", "GET", "/no-route", None),
    ("view_exception", "POST", "/total", b"{}"),
    ("after_exception", "GET", "/health", None),
)
DEFAULT_BUILD_TIMEOUT = 120


def add_build_timeout(parser):
    def positive_seconds(value):
        seconds = int(value)
        if seconds < 1:
            raise argparse.ArgumentTypeError("expected a positive number of seconds")
        return seconds

    parser.add_argument("--build-timeout", type=positive_seconds, default=DEFAULT_BUILD_TIMEOUT, metavar="SECONDS",
                        help="time limit per native compilation (default: %(default)s seconds)")


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def stage(wheelhouse, site):
    hashes = {}
    for line in (HERE / "requirements.txt").read_text().splitlines():
        url, checksum = line.split()
        name = Path(urlsplit(url).path).name
        wheel = wheelhouse / name
        if not wheel.is_file() or "--hash=sha256:" + digest(wheel) != checksum:
            raise ValueError(f"missing or incorrect wheel: {name}")
        with zipfile.ZipFile(wheel) as archive:
            archive.extractall(site)
        hashes[name] = checksum.split(":", 1)[1]
    for name in ("tiny_flask_app.py", "serve.py"):
        shutil.copyfile(HERE / name, site / name)
    return hashes


def request(port, method, route, body, *, jar=None):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=8)
    try:
        headers = {"Content-Type": "application/json"} if body is not None else {}
        if jar:
            headers["Cookie"] = "session=" + jar["session"]
        conn.request(method, route, body, headers)
        response = conn.getresponse()
        data = response.read()
        if jar is not None:
            cookies = SimpleCookie(response.getheader("Set-Cookie", ""))
            token = cookies["session"].value if "session" in cookies else ""
            parts = token.rsplit(".", 2)
            if len(parts) != 3 or not all(parts):
                raise ValueError("expected a signed Flask session cookie")
            jar["session"] = token
        allow = response.getheader("Allow", "")
        return {
            "status": response.status,
            "content_type": response.getheader("Content-Type"),
            "length": int(length) if (length := response.getheader("Content-Length")) is not None else None,
            "allow": sorted(x.strip() for x in allow.split(",") if x.strip()),
            "has_signed_cookie": bool(response.getheader("Set-Cookie")),
            "body": data.decode(),
            "sha256": hashlib.sha256(data).hexdigest(),
        }
    finally:
        conn.close()


def exercise(command, work, expected, *, prove_binary=None):
    port_file = work / "port"
    trace = work / "syscalls"
    row = {"outcome": "startup_error", "responses": {}}
    with (work / "stdout").open("wb") as stdout, (work / "stderr").open("wb") as stderr:
        try:
            if prove_binary is not None:
                command = native_identity.traced_command(prove_binary, trace)
            process = subprocess.Popen([*command, port_file], cwd=work, stdout=stdout, stderr=stderr,
                                       start_new_session=prove_binary is not None)
        except (OSError, ValueError) as exc:
            return {**row, "error": str(exc)}
        try:
            deadline = time.monotonic() + 20
            while not (port_file.exists() and (port := port_file.read_text()).isdigit()):
                if process.poll() is not None:
                    raise RuntimeError(f"server exited with status {process.returncode}")
                if time.monotonic() >= deadline:
                    raise TimeoutError("server did not publish its loopback port")
                time.sleep(0.02)
            port = int(port)
            if prove_binary is not None:
                row["outcome"] = "native_identity_error"
                tracer_pid = native_identity.require_attached(process.pid, trace, command[0])
                row["native_identity"] = native_identity.attest_socket(process.pid, prove_binary, port)
                row["native_identity"]["tracer_pid"] = tracer_pid
            row["outcome"] = "request_error"
            for name, method, path, body in CASES:
                row["responses"][name] = request(port, method, path, body)
            row["responses"]["escaped_hello"] = request(port, "GET", HELLO_PATH, None)
            jar = {}
            for name in ("session_first", "session_replay"):
                row["responses"][name] = request(port, "GET", "/visits", None, jar=jar)
            prefix, signature = jar["session"].rsplit(".", 1)
            tampered = {"session": prefix + "." + ("B" if signature.startswith("A") else "A") + signature[1:]}
            row["responses"]["session_tampered"] = request(port, "GET", "/visits", None, jar=tampered)
            row["responses"]["session_fresh"] = request(port, "GET", "/visits", None, jar={})
            if prove_binary is not None:
                row["outcome"] = "native_identity_error"
                pids, _ = native_identity.inspect_processes(process.pid, prove_binary)
                row["native_identity"]["final_pids"] = sorted(pids)
            row["outcome"] = "compare"
        except (OSError, ValueError, http.client.HTTPException, RuntimeError) as exc:
            row["error"] = str(exc)
        finally:
            if prove_binary is not None:
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            else:
                process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                if prove_binary is not None:
                    os.killpg(process.pid, signal.SIGKILL)
                else:
                    process.kill()
                process.wait()
    row["stdout"] = (work / "stdout").read_text(errors="replace")
    row["stderr"] = (work / "stderr").read_text(errors="replace")
    if prove_binary is not None and row["outcome"] == "compare":
        try:
            row["native_identity"]["trace"] = native_identity.audit_trace(trace, prove_binary, process.pid)
        except (OSError, ValueError) as exc:
            row.update(outcome="native_identity_error", error=str(exc))
    if row["outcome"] == "compare":
        reference = expected["loopback_tcp"]
        mismatch = [k for k in sorted(reference.keys() | row["responses"].keys())
                    if row["responses"].get(k) != reference.get(k)]
        cls, (key,) = expected["view_exception"]
        if row["stderr"].splitlines().count(f"{cls}: {key!r}") != 1:
            mismatch.append("view_exception traceback")
        row["outcome"] = "response_mismatch" if mismatch else "pass"
        row["mismatches"] = mismatch
    return row


def native_build(compiler, runtime, site, binary, opt, flags, timeout):
    command = [compiler, "build", site / "serve.py", "--module-root", site,
               "--runtime", runtime, "--opt-level", str(opt), "-o", binary, *flags]
    started = time.monotonic()
    p = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                         text=True, start_new_session=True)
    try:
        stdout, stderr = p.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(p.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            stdout, stderr = p.communicate(timeout=0.25)
        except subprocess.TimeoutExpired as exc:
            # A process outside the killed group may still hold a captured pipe.
            # Stop waiting for EOF and preserve the output collected so far.
            stdout = (exc.stdout or b"").decode(errors="replace")
            stderr = (exc.stderr or b"").decode(errors="replace")
            p.stdout.close()
            p.stderr.close()
            try:
                p.wait(timeout=0.25)
            except subprocess.TimeoutExpired:
                pass
        build = {"outcome": "compile_timeout", "status": None, "stdout": stdout, "stderr": stderr}
    else:
        build = {"outcome": "compile_error" if p.returncode else "compiled",
                 "status": p.returncode, "stdout": stdout, "stderr": stderr}
    return {**build, "timeout_seconds": timeout, "elapsed_seconds": time.monotonic() - started}


def native(compiler, runtime, site, work, expected, opt, flags, *, prove=False, build_timeout=DEFAULT_BUILD_TIMEOUT):
    binary = work / "server"
    build = native_build(compiler, runtime, site, binary, opt, flags, build_timeout)
    if build["outcome"] == "compile_timeout":
        return {"outcome": "compile_timeout", "build": build, "stderr": build["stderr"]}
    if build["status"]:
        return {"outcome": "compile_error", "build": build}
    return {"build": build, **exercise([binary], work, expected, prove_binary=binary if prove else None)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--wheelhouse", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--reference-only", action="store_true")
    parser.add_argument("--prove-native", action="store_true", help="attest the native executable, sockets and process tree on Linux")
    parser.add_argument("--runtime-imports", action="store_true", help="also try native runtime imports at O0 and O3")
    parser.add_argument("--compiler", type=Path)
    parser.add_argument("--runtime", type=Path)
    add_build_timeout(parser)
    args = parser.parse_args()
    expected = json.loads((HERE / "expected.json").read_text())
    if sys.implementation.name != "cpython" or sys.version.split()[0] != expected["python"]:
        parser.error(f"run the gate with CPython {expected['python']}")
    if not args.reference_only and not (args.compiler and args.runtime):
        parser.error("supply --compiler and --runtime, or explicitly select --reference-only")
    if args.reference_only and (args.prove_native or args.runtime_imports):
        parser.error("--prove-native and --runtime-imports require a native compiler and runtime")
    report = {"python": sys.version.split()[0], "app_sha256": digest(HERE / "tiny_flask_app.py"), "modes": {}}
    if report["app_sha256"] != expected["app_sha256"]:
        parser.error("the app differs from the recorded reference")
    with tempfile.TemporaryDirectory() as temp:
        root = Path(temp)
        site = root / "site"
        site.mkdir()
        try:
            report["wheels"] = stage(args.wheelhouse, site)
        except (OSError, ValueError, zipfile.BadZipFile) as exc:
            parser.exit(2, f"could not stage Flask: {exc}\n")
        # The archived CPython fixture loaded MarkupSafe's optional C extension.
        speedups = subprocess.run([sys.executable, "-ESB", "-c",
                                   "from markupsafe import _speedups; print(_speedups.__file__)"],
                                  cwd=site, capture_output=True, text=True)
        extension = site / "markupsafe" / expected["speedups"]
        if speedups.returncode or Path(speedups.stdout.strip()) != extension:
            parser.exit(2, f"could not load pinned MarkupSafe extension: {speedups.stderr}\n")
        report["cpython_speedups"] = extension.name
        work = root / "reference"
        work.mkdir()
        # -E ignores PYTHON*; -S omits site-packages. The staged script directory supplies all seven wheels.
        report["modes"]["cpython"] = exercise([sys.executable, "-ESB", site / "serve.py"], work, expected)
        if not args.reference_only:
            compiler, runtime = args.compiler.resolve(), args.runtime.resolve()
            report.update(compiler_sha256=digest(compiler), runtime_sha256=digest(runtime))
            modes = [("normal", []), ("reachable", ["--reachable-only"])]
            if args.runtime_imports:
                modes.append(("runtime-imports", ["--runtime-imports"]))
            for name, flags in modes:
                for opt in (0, 3):
                    mode = f"{name}-O{opt}"
                    work = root / mode
                    work.mkdir()
                    row = native(compiler, runtime, site, work, expected, opt, flags,
                                 prove=args.prove_native, build_timeout=args.build_timeout)
                    report["modes"][mode] = row
                    if args.prove_native:
                        row["verified_native_responses"] = len(row["responses"]) if row["outcome"] == "pass" else 0
                    if args.prove_native and (trace := work / "syscalls").is_file():
                        directory = args.output.parent / (args.output.stem + "-traces")
                        directory.mkdir(parents=True, exist_ok=True)
                        copy = directory / (mode + ".strace")
                        shutil.copyfile(trace, copy)
                        row["trace_file"] = str(copy.relative_to(args.output.parent))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    for name, row in report["modes"].items():
        print(f"{name}: {row['outcome']}")
    return int(any(row["outcome"] != "pass" for row in report["modes"].values()))


if __name__ == "__main__":
    raise SystemExit(main())
