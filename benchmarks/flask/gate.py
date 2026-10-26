"""Compare a Flask application over real TCP in CPython and Imre."""
import argparse
import hashlib
import http.client
import json
import shutil
import subprocess
import sys
import tempfile
import time
import zipfile
from pathlib import Path
from urllib.parse import urlsplit

HERE = Path(__file__).resolve().parent
CASES = (
    ("health", "GET", "/health", None),
    ("bigint", "POST", "/total", json.dumps({"values": [2**95, 3, -2]}).encode()),
    ("missing", "GET", "/no-route", None),
    ("view_exception", "POST", "/total", b"{}"),
    ("after_exception", "GET", "/health", None),
)


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


def request(port, method, route, body):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=8)
    try:
        headers = {"Content-Type": "application/json"} if body is not None else {}
        conn.request(method, route, body, headers)
        response = conn.getresponse()
        data = response.read()
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


def exercise(command, work, expected):
    port_file = work / "port"
    row = {"outcome": "startup_error", "responses": {}}
    with (work / "stdout").open("wb") as stdout, (work / "stderr").open("wb") as stderr:
        try:
            process = subprocess.Popen([*command, port_file], cwd=work, stdout=stdout, stderr=stderr)
        except OSError as exc:
            return {**row, "error": str(exc)}
        try:
            deadline = time.monotonic() + 20
            while not (port_file.exists() and (port := port_file.read_text()).isdigit()):
                if process.poll() is not None:
                    raise RuntimeError(f"server exited with status {process.returncode}")
                if time.monotonic() >= deadline:
                    raise TimeoutError("server did not publish its loopback port")
                time.sleep(0.02)
            row["outcome"] = "request_error"
            for name, method, path, body in CASES:
                row["responses"][name] = request(int(port), method, path, body)
            row["outcome"] = "compare"
        except (OSError, ValueError, http.client.HTTPException, RuntimeError) as exc:
            row["error"] = str(exc)
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
    row["stdout"] = (work / "stdout").read_text(errors="replace")
    row["stderr"] = (work / "stderr").read_text(errors="replace")
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


def native(compiler, runtime, site, work, expected, opt, reachable):
    binary = work / "server"
    command = [compiler, "build", site / "serve.py", "--module-root", site,
               "--runtime", runtime, "--opt-level", str(opt), "-o", binary]
    if reachable:
        command.append("--reachable-only")
    try:
        p = subprocess.run(command, capture_output=True, text=True, timeout=120)
    except subprocess.TimeoutExpired as exc:
        return {"outcome": "compile_timeout", "stderr": (exc.stderr or b"").decode(errors="replace")}
    build = {"status": p.returncode, "stdout": p.stdout, "stderr": p.stderr}
    if p.returncode:
        return {"outcome": "compile_error", "build": build}
    return {"build": build, **exercise([binary], work, expected)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--wheelhouse", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--reference-only", action="store_true")
    parser.add_argument("--hosted-launcher", type=Path, help="run Imre LLVM views with CPython Flask")
    parser.add_argument("--compiler", type=Path)
    parser.add_argument("--runtime", type=Path)
    args = parser.parse_args()
    expected = json.loads((HERE / "expected.json").read_text())
    if sys.implementation.name != "cpython" or sys.version.split()[0] != expected["python"]:
        parser.error(f"run the gate with CPython {expected['python']}")
    if bool(args.compiler) != bool(args.runtime):
        parser.error("--compiler and --runtime must be supplied together")
    if args.reference_only and args.hosted_launcher:
        parser.error("--reference-only excludes the hosted configuration")
    if not (args.reference_only or args.hosted_launcher or args.compiler):
        parser.error("select --compiler and --runtime, --hosted-launcher, or --reference-only")
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
        if args.hosted_launcher:
            work = root / "imre-views-on-cpython"
            work.mkdir()
            hosted = exercise(
                [sys.executable, "-ISB", args.hosted_launcher.resolve(), site], work, expected)
            if hosted["outcome"] == "pass":
                marker = 'tiny_flask_app.py", line '
                def view_frames(row):
                    return [line.split(marker)[-1] for line in row["stderr"].splitlines() if marker in line]
                frames = view_frames(report["modes"]["cpython"])
                if not frames or view_frames(hosted) != frames:
                    hosted["outcome"] = "response_mismatch"
                    hosted["mismatches"].append("view exception source location")
            report["modes"]["imre-views-on-cpython"] = hosted
        if args.compiler and not args.reference_only:
            compiler, runtime = args.compiler.resolve(), args.runtime.resolve()
            report.update(compiler_sha256=digest(compiler), runtime_sha256=digest(runtime))
            for reachable in (False, True):
                for opt in (0, 3):
                    mode = f"{'reachable' if reachable else 'normal'}-O{opt}"
                    work = root / mode
                    work.mkdir()
                    report["modes"][mode] = native(compiler, runtime, site, work, expected, opt, reachable)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    for name, row in report["modes"].items():
        print(f"{name}: {row['outcome']}")
    return int(any(row["outcome"] != "pass" for row in report["modes"].values()))


if __name__ == "__main__":
    raise SystemExit(main())
