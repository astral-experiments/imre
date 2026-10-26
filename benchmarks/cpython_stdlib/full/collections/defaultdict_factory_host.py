"""Run the original factory assertion with an Imre constructor and CPython unittest."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import roles
from suite_inventory import load_suite, verify_files
from suite_native import Toolchain, method_bytes, run_in_fixtures

CASE = "test.test_defaultdict.TestDefaultDict.test_callable_arg"
PROGRAM = b"""from collections import defaultdict
import sys

factory: dict[str, int] = {}
i = 2
while i < len(sys.argv):
    factory[sys.argv[i]] = int(sys.argv[i + 1])
    i += 2
if sys.argv[1] == "call":
    defaultdict(factory)
elif sys.argv[1] == "wrong-exception":
    raise ValueError("assertion control")
"""


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + name, type=Path, required=True)
    p.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    p.add_argument("--compiler-revision", required=True)
    args = p.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        p.error("use a test-enabled CPython 3.14.5")
    from test import test_defaultdict

    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    verify_files(HERE / "upstream", manifest["files"])
    verify_files(Path(test_defaultdict.__file__).resolve().parents[2], manifest["files"])
    source = HERE / "upstream/Lib/test/test_defaultdict.py"
    method = hashlib.sha256(method_bytes(source, "TestDefaultDict", "test_callable_arg")).hexdigest()
    toolchain = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    with tempfile.TemporaryDirectory() as work:
        binary = toolchain.build(PROGRAM, Path(work))

        def run(mode):
            calls = []

            def constructor(factory, /):
                if type(factory) is not dict or not all(type(k) is str and type(v) is int for k, v in factory.items()):
                    raise AssertionError("unsupported operand transport")
                pairs = [str(item) for pair in factory.items() for item in pair]
                proc = subprocess.run([binary, mode, *pairs], text=True, capture_output=True, timeout=30)
                calls.append({"entries": len(factory), "exit": proc.returncode, "stderr": proc.stderr})
                if proc.stdout:
                    raise AssertionError(proc.stdout)
                if proc.returncode == 0 and not proc.stderr:
                    return
                category, separator, message = proc.stderr.rstrip("\n").partition(": ")
                exceptions = {"TypeError": TypeError, "ValueError": ValueError}
                if proc.returncode != 1 or not separator or category not in exceptions or "\n" in message:
                    raise AssertionError((proc.returncode, proc.stderr))
                raise exceptions[category](message)

            def execute(case, original):
                with patch.object(test_defaultdict, "defaultdict", constructor):
                    original()
                if len(calls) != 1:
                    raise AssertionError(calls)
                return {"observation": {
                    "roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                    "evidence": {"method_sha256": method, "opt_level": args.opt_level,
                                 "operation": "The linked Imre child builds the supplied dictionary and calls collections.defaultdict.",
                                 "fixture": "Original CPython module, class and instance hooks, method and assertRaises.",
                                 "native_calls": {"defaultdict": len(calls)}, "child_results": calls},
                }}
            results = run_in_fixtures(load_suite("test.test_defaultdict"), {CASE: execute})
            return results, calls

        (tests, failures, skips), actual = run("call")
        if failures or skips or CASE not in tests or len(actual) != 1:
            raise AssertionError((failures, skips, actual))
        controls = {}
        for mode in ("return", "wrong-exception"):
            (_, failures, skips), calls = run(mode)
            if skips or len(failures) != 1 or len(calls) != 1:
                raise AssertionError((mode, failures, skips, calls))
            controls[mode] = {"original_test_failed": True, "child_results": calls}
    args.result.write_text(json.dumps({"cpython_commit": manifest["commit"], "opt_level": args.opt_level,
                                       "compiler_revision": args.compiler_revision, "tests": tests,
                                       "failures": [], "skips": [], "controls": controls}, indent=2) + "\n")


if __name__ == "__main__":
    main()
