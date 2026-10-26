"""Run complete OS environment and path-conversion methods under their upstream fixtures."""
import argparse
from collections import Counter
from contextlib import chdir
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import roles
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import NativeCompileError, Toolchain, native_method, run_in_fixtures

CASES = ("TestPEP519.test_return_bytes", "TestPEP519.test_return_string", "EnvironTests.test_items",
         "FSEncodingTests.test_nop", "FSEncodingTests.test_identity")


def getcwd_body(case, method, binary, tools, root, path):
    blocker = None
    if case._testMethodName == "test_getcwd":
        directory = root / "original-getcwd"
        directory.mkdir()
        try:
            run = native_method(tools, directory, path, "MiscTests", case.id(),
                                "import os\nimport unittest\n\nclass NativeCase(unittest.TestCase):\n",
                                operation=("imre", "os.getcwd"))
        except NativeCompileError as error:
            blocker = "MiscTests.test_getcwd, self.assertIsInstance(cwd, str): " + str(error).strip().split("Imre: ")[-1]
        else:
            result = run(case, method)
            result.update(status="native_body_host_fixture", host_fixtures=["test_os.tearDownModule"])
            return result
    count = 0
    host = Counter()
    def native():
        nonlocal count
        count += 1
        run = subprocess.run([binary], capture_output=True, check=True, timeout=30)
        if run.stderr or not run.stdout.endswith(b"\n"):
            raise AssertionError(f"incomplete native getcwd: {run.stderr!r}")
        return run.stdout[:-1].decode()

    def observed(name, function):
        def call(*args):
            host[name] += 1
            return function(*args)
        return call
    view = SimpleNamespace(getcwd=native, mkdir=observed("mkdir", os.mkdir),
                           chdir=observed("chdir", os.chdir),
                           path=SimpleNamespace(sep=os.path.sep, join=observed("path.join", os.path.join)))
    with patch.object(sys.modules[type(case).__module__], "os", view):
        method()
    if not count:
        raise AssertionError("no native getcwd")
    row = {"status": "mixed" if host else "host_driven_native", "native_getcwd_calls": count,
           "host_calls": dict(host), "host_fixtures": ["test_os.tearDownModule"]}
    row["observation"] = {
        "roles": roles(body="cpython", assertions="cpython", operation="mixed" if host else "imre", fixture="cpython"),
        "evidence": {"operation": "os.getcwd", "fixture": "; ".join(row["host_fixtures"]),
                     "opt_level": tools.level, "native_call_count": count, "host_calls": dict(host)}}
    if blocker:
        row["native_body_blocker"] = blocker
    return row


def check(case, method, toolchain, root, path):
    owner = type(case).__name__
    if owner == "EnvironTests":
        # Read the original expected fixture, independently of the environment
        # populated by setUp. The native child inherits that real environment.
        fixture = f'''import os

class NativeCase:
    def _reference(self) -> dict[str, str]:
        return {case._reference()!r}

    def assertEqual(self, actual: str | None, expected: str) -> None:
        assert actual is not None and actual == expected
'''
        hooks = ["EnvironTests.setUp and tearDown", "EnvironTests._reference", "test_os.tearDownModule"]
    elif owner == "FSEncodingTests":
        fixture = "import os\nimport unittest\n\nclass NativeCase(unittest.TestCase):\n"
        hooks = ["test_os.tearDownModule"]
        if case._testMethodName == "test_nop":
            fixture = '''import os

class NativeCase:
    def assertEqual(self, actual: str | bytes, expected: str | bytes) -> None:
        if isinstance(actual, bytes):
            assert isinstance(expected, bytes) and actual == expected
        else:
            assert isinstance(expected, str) and actual == expected
'''
    else:
        ty = "bytes" if case._testMethodName == "test_return_bytes" else "str"
        fixture = f'''import os

class NativeCase:
    def fspath(self, path: {ty}) -> {ty}:
        return os.fspath(path)

    def assertEqual(self, expected: {ty}, actual: {ty}) -> None:
        assert expected == actual
'''
        hooks = ["test_os.tearDownModule"]
    directory = root / case.id()
    directory.mkdir()
    operation = {"EnvironTests": "os.environ.get", "FSEncodingTests": "os.fsencode and os.fsdecode",
                 "TestPEP519": "os.fspath"}[owner]
    run = native_method(toolchain, directory, path, owner, case.id(), fixture, operation=("imre", operation))
    evidence = run(case, method)
    evidence.update(status="native_body_host_fixture", host_fixtures=hooks)
    return evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--opt-level", choices=(0, 3), type=int, required=True)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    manifest = json.loads((HERE / "os.json").read_bytes())
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use test-enabled CPython 3.14.5")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    toolchain = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), args.opt_level)
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    path = HERE / "upstream/Lib/test/test_os.py"
    with tempfile.TemporaryDirectory() as tmp, chdir(tmp):
        wd = Path(tmp) / "getcwd"
        wd.mkdir()
        binary = toolchain.build(b"import os\nprint(os.getcwd())\n", wd)
        callbacks = {f"test.test_os.{name}":
                     (lambda case, method: check(case, method, toolchain, Path(tmp), path))
                     for name in CASES}
        for name in ("test_getcwd", "test_getcwd_long_path"):
            callbacks[f"test.test_os.MiscTests.{name}"] = lambda case, method: getcwd_body(case, method, binary, toolchain, Path(tmp), path)
        evidence, failures, skips = run_in_fixtures(load_suite(manifest["module"]), callbacks)
    rows = {}
    for uid, entries in evidence.items():
        rows[uid], = entries
    for uid, reason in skips:
        rows[uid] = {"reference_skip": reason}
    for uid, diagnostic in failures:
        rows[uid] = {"status": "failed", "diagnostic": diagnostic}
    if callbacks.keys() - rows.keys():
        raise ValueError(f"methods not executed: {callbacks.keys() - rows.keys()}")
    for uid, status in rows.items():
        print(uid, status, flush=True)
    args.result.write_text(json.dumps({"cpython_commit": manifest["commit"], "opt_level": args.opt_level,
                                      "cases": rows}, indent=2) + "\n")
    return int(bool(failures))


if __name__ == "__main__":
    sys.exit(main())
