"""Compile unchanged subprocess test methods with native fixtures and assertions."""

import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
# The subprocess operation, method body, and assertions all run in the binary.
# sys.executable selects a real oracle child for upstream's literal -c programs.
METHODS = {"ContextManagerTests": {"test_returncode": 1}}
PREAMBLE = b"""import subprocess
import sys

class Case:
    checks: int

    def __init__(self) -> None:
        self.checks = 0

    def assertEqual(self, left: int | None, right: int) -> None:
        if left is None:
            raise ValueError("unexpected None")
        if left != right:
            raise ValueError("unequal status")
        self.checks += 1

"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + flag, type=Path, required=True)
    parser.add_argument("--opt-level", choices=("0", "3"), required=True)
    args = parser.parse_args()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    source = HERE / "upstream/Lib/test/test_subprocess.py"
    data = source.read_bytes()
    blob = hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()
    if blob != manifest["files"]["Lib/test/test_subprocess.py"]:
        raise ValueError(f"modified upstream test: {source}")
    tree = ast.parse(data)
    lines = data.splitlines(keepends=True)
    results = {}
    with tempfile.TemporaryDirectory() as tmp:
        for cls, methods in METHODS.items():
            case = next(c for c in tree.body if isinstance(c, ast.ClassDef) and c.name == cls)
            for method, assertions in methods.items():
                node = next(n for n in case.body if isinstance(n, ast.FunctionDef) and n.name == method)
                if node.decorator_list:
                    raise ValueError(f"{cls}.{method} requires a decorator fixture")
                body = b"".join(lines[node.lineno - 1:node.end_lineno])
                entry = Path(tmp) / (method + ".py")
                exe = entry.with_suffix("")
                entry.write_bytes(PREAMBLE + body + f"\ncase = Case()\ncase.{method}()\nprint(case.checks)\n".encode())
                build = subprocess.run([args.imre.resolve(), "build", entry, "--opt-level", args.opt_level,
                                        "--clang", args.clang.resolve(), "--runtime", args.runtime.resolve(),
                                        "-o", exe], capture_output=True, text=True, timeout=120)
                run = build if build.returncode else subprocess.run(
                    [exe], env={**os.environ, "IMRE_PYTHON": sys.executable},
                    capture_output=True, text=True, timeout=30)
                key = f"test.test_subprocess.{cls}.{method}"
                if run.returncode or run.stdout != f"{assertions}\n" or run.stderr:
                    raise AssertionError((key, run.returncode, run.stdout, run.stderr))
                results[key] = {"assertions": assertions, "body_sha256": hashlib.sha256(body).hexdigest()}
    args.result.write_text(json.dumps({"opt_level": args.opt_level, "fully_native": results}, indent=2) + "\n")


if __name__ == "__main__":
    main()
