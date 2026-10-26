"""Compare the public http root and exact Werkzeug excerpts with ordinary CPython."""
import argparse
import ast
from hashlib import sha256
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from zipfile import ZipFile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import verify_files
from suite_native import Toolchain

SUITES = ("httplib", "httpservers", "http_cookies", "http_cookiejar")


def manifests():
    for suite in SUITES:
        manifest = json.loads((HERE / f"{suite}.json").read_bytes())
        verify_files(HERE / "upstream", manifest["files"])
        yield suite, manifest


def werkzeug_source(wheel):
    root = HERE / "werkzeug"
    receipt = json.loads((root / "UPSTREAM.json").read_bytes())
    for name, row in receipt["sources"].items():
        if sha256((root / name).read_bytes()).hexdigest() != row["excerpt_sha256"]:
            raise ValueError(f"changed Werkzeug excerpt: {name}")
    if wheel:
        if sha256(wheel.read_bytes()).hexdigest() != receipt["sha256"]:
            raise ValueError("Werkzeug wheel SHA-256 mismatch")
        with ZipFile(wheel) as archive:
            for name, row in receipt["sources"].items():
                data = archive.read(row["path"])
                first, last = row["lines"]
                if (sha256(data).hexdigest() != row["source_sha256"] or
                        b"".join(data.splitlines(keepends=True)[first - 1:last]) != (root / name).read_bytes()):
                    raise ValueError(f"wheel differs from {name}")
    return (b"from http import HTTPStatus\n" + (root / "HTTP_STATUS_CODES.inc").read_bytes()
            + b"\nclass Response:\n" + (root / "_clean_status.inc").read_bytes()
            + b"\n" + (root / "inputs.py").read_bytes())


def responses_source():
    source = (HERE / "upstream/Lib/http/server.py").read_text()
    [handler] = [node for node in ast.parse(source).body
                 if isinstance(node, ast.ClassDef) and node.name == "BaseHTTPRequestHandler"]
    [responses] = [node for node in handler.body if isinstance(node, ast.Assign)
                   and any(isinstance(target, ast.Name) and target.id == "responses" for target in node.targets)]
    # Keep the class's original comprehension and supply just its HTTPStatus dependency.
    return ("from http import HTTPStatus\n" + ast.get_source_segment(source, responses)
            + "\n" + (HERE / "responses.py").read_text()).encode()


def run(command):
    process = subprocess.run(command, capture_output=True, timeout=45)
    if process.returncode:
        raise RuntimeError(f"{command[0]} exited {process.returncode}: {process.stderr.decode()}")
    if process.stderr:
        raise RuntimeError(process.stderr.decode())
    return process.stdout


def options(description, *, wheel=False):
    parser = argparse.ArgumentParser(description=description)
    parser.add_argument("--compiler", required=True, type=Path)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--clang", type=Path, default=Path(shutil.which("clang") or "clang"))
    parser.add_argument("--output", type=Path)
    if wheel:
        parser.add_argument("--wheel", type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use ordinary CPython 3.14.5")
    return args


def toolchains(args):
    for level in (0, 3):
        for reachable in (False, True):
            yield Toolchain(args.compiler, args.runtime, args.clang, level, reachable)


def main():
    args = options(__doc__, wheel=True)
    import http
    for _, manifest in manifests():
        verify_files(Path(http.__file__).resolve().parent.parent,
                     {name.removeprefix("Lib/"): blob for name, blob in manifest["files"].items()
                      if name.startswith("Lib/http/")})
    programs = {"http": (HERE / "operations.py").read_bytes(), "werkzeug": werkzeug_source(args.wheel),
                "responses": responses_source()}
    report = []
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        for name, source in programs.items():
            ref = work / "reference.py"
            ref.write_bytes(source)
            expected = run([sys.executable, "-B", ref])
            if name == "werkzeug" and args.wheel:
                # Load this actual wheel first. Its dependency MarkupSafe must be installed.
                installed = ("import sys\nsys.path.insert(0, sys.argv[1])\nfrom http import HTTPStatus\n"
                             "from werkzeug.sansio import response\nfrom werkzeug.sansio.response import Response\n"
                             "assert response.__file__.startswith(sys.argv[1] + '/')\n")
                ref.write_bytes(installed.encode() + (HERE / "werkzeug/inputs.py").read_bytes())
                if run([sys.executable, "-B", ref, args.wheel.resolve()]) != expected:
                    raise AssertionError("CPython wheel and excerpt fixture disagree")
            for tool in toolchains(args):
                binary = tool.build(source, work)
                if run([binary]) != expected:
                    raise AssertionError(f"{name} differs at O{tool.level}, reachable_only={tool.reachable_only}")
                report.append({"fixture": name, "opt_level": tool.level, "reachable_only": tool.reachable_only,
                               "source_sha256": sha256(source).hexdigest(), "stdout_sha256": sha256(expected).hexdigest(),
                               "lines": len(expected.splitlines())})
    if args.output:
        args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(f"{len(report)} native comparisons matched ordinary CPython 3.14.5")


if __name__ == "__main__":
    main()
