"""Run complete OS environment and path-conversion methods under their upstream fixtures."""
import argparse
import ast
from collections import Counter
from contextlib import chdir
import hashlib
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
from suite_native import NativeCompileError, Toolchain, method_bytes, native_method, run_in_fixtures

CASES = ("TestPEP519.test_return_bytes", "TestPEP519.test_return_string", "EnvironTests.test_items",
         "FSEncodingTests.test_nop", "FSEncodingTests.test_identity", "TestPEP519.test_argument_required")


def pep519_codecs(case, method, binary, directory, source):
    """Run the release method and its FakePath in the host; convert each returned path in Imre."""
    module = sys.modules[type(case).__module__]
    host = Counter()
    native = []
    operation = case.fspath
    descriptor = module.FakePath.__fspath__

    def fspath(value):
        host["os." + operation.__name__] += 1
        return operation(value)

    def protocol(value):
        host["FakePath.__fspath__"] += 1
        return descriptor(value)

    def convert(name, value):
        spelling = type(value).__fspath__(value)
        if type(spelling) is bytes:
            (directory / "input").write_bytes(spelling)
        elif type(spelling) is str:
            (directory / "input").write_text(spelling, encoding="utf-8")
        else:
            raise AssertionError("unexpected upstream FakePath payload")
        result = subprocess.run([binary, name, type(spelling).__name__], cwd=directory,
                                capture_output=True, text=True, timeout=30, check=True)
        if result.stderr:
            raise AssertionError(result.stderr)
        native.append(name)
        return ast.literal_eval(result.stdout)

    view = SimpleNamespace(fsencode=lambda value: convert("fsencode", value),
                           fsdecode=lambda value: convert("fsdecode", value))
    with patch.object(module, "os", view), patch.object(module.FakePath, "__fspath__", protocol), \
         patch.object(case, "fspath", fspath):
        method()
    if Counter(native) != {"fsencode": 2, "fsdecode": 2} or host["FakePath.__fspath__"] != 6:
        raise AssertionError((native, host))
    return {"status": "mixed", "native_calls": native, "host_calls": dict(host),
            "method_sha256": hashlib.sha256(method_bytes(source, "TestPEP519", case._testMethodName)).hexdigest(),
            "host_fixtures": ["test_os.tearDownModule", "original FakePath, self.fspath, loop and assertions"],
            "native_body_blocker": "FakePath references BaseException in its descriptor; the method iterates a heterogeneous str/bytes tuple."}


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
    elif case._testMethodName == "test_argument_required":
        fixture = "import os\nimport unittest\n\nclass NativeCase(unittest.TestCase):\n    fspath = staticmethod(os.fspath)\n"
        hooks = ["test_os.tearDownModule"]
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
        codecs = Path(tmp) / "pep519"
        codecs.mkdir()
        codec_binary = toolchain.build((HERE / "pep519_codec_calls.py").read_bytes(), codecs)
        for owner in ("TestPEP519", "TestPEP519PurePython"):
            callbacks[f"test.test_os.{owner}.test_fsencode_fsdecode"] = (
                lambda case, method: pep519_codecs(case, method, codec_binary, codecs, path))
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
