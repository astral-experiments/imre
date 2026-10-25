"""Compare the tuple from threading_helper and all four unchanged itertools entry modes."""
import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
PIN = "5607950ef232dad16d75c0cf53101d9649d89115"
HELPER = "Lib/test/support/threading_helper.py"
BLOB = "cf87233f0e2e938a9f2e7d7906844ca009416432"


def display_line(source, method):
    node = next(n for n in ast.parse(source).body if isinstance(n, ast.FunctionDef) and n.name == method)
    field, = [n for n in ast.walk(node) if isinstance(n, ast.Tuple) and
              ast.get_source_segment(source, n) == "(func, *args)"]
    return field.lineno


def startup(args, git):
    command = [sys.executable, "-B", HERE.parent / "check_suite_startup.py", "--compiler", args.imre]
    if git:
        command += ["--cpython-git", git]
    proc = subprocess.run([*command, "test.test_itertools"], capture_output=True, timeout=150)
    if proc.stderr:
        raise RuntimeError(proc.stderr.decode())
    report = json.loads(proc.stdout)
    rows = report["checks"]
    assert report["cpython_commit"] == PIN and report["tests_executed"] == 0 and report["operation"] == "check"
    assert sorted((r["entry"], r["reachable_only"]) for r in rows) == [
        ("import", False), ("import", True), ("runner", False), ("runner", True)]
    assert proc.returncode == int(any(r["exit"] for r in rows))
    assert all("threading_helper.py:277:63" not in r["diagnostic"] for r in rows), rows
    return report


def reachability_controls(args, root):
    helper = root / "spread_helper.py"
    helper.write_text(
        'def dormant(args: list[int]) -> None:\n'
        '    captured = (7, *args)\n'
        '    eval("0")\n\n'
        'def used(args: list[int]) -> int:\n'
        '    return len((1, *args))\n')
    entry = root / "entry.py"
    entry.write_text("from spread_helper import used\nprint(used([1, 2]))\n")
    def check(path):
        proc = subprocess.run([args.imre, "check", path, "--module-root", root],
                              capture_output=True, timeout=30)
        assert proc.returncode == 1
        return (proc.stdout + proc.stderr).decode().strip().removeprefix(str(root) + "/")
    unused = check(entry)
    assert unused.startswith("spread_helper.py:3:5:"), unused
    reachable = []
    for level in ("0", "3"):
        exe = root / "reachable"
        subprocess.run([args.imre, "build", entry, "--module-root", root, "--reachable-only",
                        "--no-cache", "--quiet", "--opt-level", level, "--runtime", args.runtime,
                        "--clang", args.clang, "-o", exe], check=True, timeout=120)
        result = subprocess.run([exe], capture_output=True, check=True, timeout=30)
        assert result.stdout == b"3\n" and not result.stderr
        reachable.append({"opt_level": level, "stdout": result.stdout.decode()})
    unrelated = root / "bad.py"
    unrelated.write_text("def unrelated(n: int) -> None:\n    for item in n:\n        print(item)\nunrelated(7)\n")
    invalid = check(unrelated)
    assert invalid.startswith("bad.py:2:9:") and "iteration requires" in invalid, invalid
    return {"whole_file_diagnostic": unused, "reachable_native": reachable,
            "unrelated_iteration_diagnostic": invalid}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("imre", "runtime", "clang", "cpython-git", "result"):
        parser.add_argument("--" + option, type=Path, required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        raise RuntimeError("Run the reference with CPython 3.14.5")
    data = (HERE.parent / "shared" / "upstream" / HELPER).read_bytes()
    blob = hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()
    upstream = subprocess.check_output(["git", "-C", args.cpython_git, "show", f"{PIN}:{HELPER}"])
    assert data == upstream and blob == BLOB
    source = HERE / "tuple_spreads.py"
    assert display_line(data.decode(), "run_concurrently") == 277
    assert display_line(source.read_text(), "call_prefix")
    reference = subprocess.run([sys.executable, "-B", source], capture_output=True, check=True)
    controls = []
    with tempfile.TemporaryDirectory(prefix="imre-tuple-displays-") as tmp:
        exe = Path(tmp) / "display"
        for reachable in (False, True):
            for level in ("0", "3"):
                command = [args.imre, "build", source, "--no-cache", "--quiet", "--opt-level", level,
                           "--runtime", args.runtime, "--clang", args.clang, "-o", exe]
                if reachable:
                    command.append("--reachable-only")
                subprocess.run(command, check=True, timeout=120)
                native = subprocess.run([exe], capture_output=True, check=True, timeout=30)
                assert (native.stdout, native.stderr) == (reference.stdout, reference.stderr)
                controls.append({"opt_level": level, "reachable_only": reachable,
                                 "lines": len(native.stdout.splitlines()),
                                 "stdout_sha256": hashlib.sha256(native.stdout).hexdigest()})
        reachability = reachability_controls(args, Path(tmp))
    vendored, pinned = (startup(args, git) for git in (None, args.cpython_git))
    assert vendored["checks"] == pinned["checks"]
    assert vendored["compiler_sha256"] == pinned["compiler_sha256"]
    result = {"cpython_commit": PIN, "helper": HELPER, "git_blob": blob, "line": 277,
              "expression": "(func, *args)", "native_controls": controls,
              "reference_stdout": reference.stdout.decode().splitlines(), "reachability": reachability,
              "source": "vendored and pinned Git Lib/test", "whole_suite": pinned,
              "credit": "Supplemental native controls; startup checks executed zero upstream test bodies."}
    args.result.write_text(json.dumps(result, indent=2) + "\n")
    for row in controls:
        print(f'O{row["opt_level"]} reachable={row["reachable_only"]}: {row["lines"]} native lines')
    for row in pinned["checks"]:
        print(f'{row["entry"]} reachable={row["reachable_only"]}: {row["diagnostic"]}')


if __name__ == "__main__":
    main()
