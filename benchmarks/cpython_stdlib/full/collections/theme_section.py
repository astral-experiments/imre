"""Run mapping operations on CPython's unchanged, dataclass-backed argparse theme."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile


BLOB = "d6673f6692f76151b3b226ebe4d7976456a257e5"
DRIVER = '''from collections.abc import Mapping
from stock_colorize import Argparse

theme = Argparse(usage="use", prog="program")
print(isinstance(theme, Mapping))
print(theme["usage"])
print(theme.get("prog") == "program")
print(theme.get("absent", "fallback") == "fallback")
print("usage" in theme)
print("absent" not in theme)
print(len(theme))
print(list(theme)[:2])
try:
    theme["absent"]
except KeyError as error:
    print(error)
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cpython-source", type=Path, required=True)
    parser.add_argument("--compiler-revision")
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=name == "result")
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        parser.error("use the pinned CPython 3.14.5 interpreter")
    native = [args.imre, args.runtime, args.clang, args.compiler_revision]
    if any(native) and not all(native):
        parser.error("pass --imre, --runtime, --clang and --compiler-revision together")
    source = (args.cpython_source / "Lib/_colorize.py").read_bytes()
    if hashlib.sha1(b"blob " + str(len(source)).encode() + b"\0" + source).hexdigest() != BLOB:
        parser.error("Lib/_colorize.py differs from the CPython 3.14.5 Git blob")
    report = {"cpython_commit": "5607950ef232dad16d75c0cf53101d9649d89115",
              "colorize_blob": BLOB, "driver_sha256": hashlib.sha256(DRIVER.encode()).hexdigest(),
              "compiler_revision": args.compiler_revision, "upstream_test_occurrences": 0, "modes": {}}
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        # Use the source under a distinct import name so a sealed stdlib binding
        # cannot replace the stock ThemeSection and its dataclass subclass.
        (work / "stock_colorize.py").write_bytes(source)
        entry = work / "entry.py"
        entry.write_text(DRIVER)
        oracle = subprocess.run([sys.executable, "-E", "-B", entry], cwd=work, capture_output=True, check=True)
        report["reference"] = oracle.stdout.decode()
        if args.imre:
            imre, runtime, clang = (p.resolve() for p in (args.imre, args.runtime, args.clang))
            for level in (0, 3):
                for reachable in (False, True):
                    exe = work / "case"
                    command = [imre, "build", entry, "--module-root", work, "--no-cache", "--runtime", runtime,
                               "--clang", clang, "--opt-level", str(level), "-o", exe]
                    if reachable:
                        command.append("--reachable-only")
                    build = subprocess.run(command, cwd=work, capture_output=True)
                    if build.returncode:
                        row = {"outcome": "compile-blocked", "module_operations": "not-run",
                               "compile_exit": build.returncode,
                               "diagnostic": build.stderr.decode().replace(tmp, "<work>")}
                    else:
                        result = subprocess.run([exe], cwd=work, capture_output=True)
                        row = {"outcome": "matched" if (result.returncode, result.stdout, result.stderr) ==
                               (0, oracle.stdout, oracle.stderr) else "failed", "exit": result.returncode,
                               "module_operations": "native-executable",
                               "stdout": result.stdout.decode(), "stderr": result.stderr.decode()}
                    report["modes"][f"O{level}-reachable-{int(reachable)}"] = row
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
