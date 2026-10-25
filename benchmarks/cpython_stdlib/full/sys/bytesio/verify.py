"""Compare native BytesIO snapshots and run the original CBytesIOTest method."""
import argparse
from collections import Counter
from contextlib import ExitStack
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE.parents[2]), str(HERE.parents[1] / "doctest" / "stringio")]
from stream_fixture import Stream
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import NativeCompileError, Toolchain, method_bytes, native_method, run_in_fixtures

SOURCE = "Lib/test/test_memoryio.py"
BLOB = "482b183da23ffa259664e52592021d5c3c12d34d"
MODULE = "test.test_memoryio"
CASE = MODULE + ".CBytesIOTest.test_getvalue"
OWNER = "MemoryTestMixin"
FIXTURE = """from io import BytesIO
import unittest

class NativeCase(unittest.TestCase):
    def buftype(self, text: str) -> bytes:
        return text.encode("ascii")

    def ioclass(self, initial: bytes) -> BytesIO:
        return BytesIO(initial)
"""


class BinaryStream(Stream):
    def __init__(self, binary, stack, calls, initial):
        self.child = subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                      stderr=subprocess.PIPE, text=True, encoding="utf-8")
        self.calls = calls
        stack.callback(self.finish)
        self.send(len(initial), *initial)
        if self.receive() != "ready":
            raise AssertionError("native BytesIO construction failed")
        calls["BytesIO"] += 1


def host_executor(binary, source):
    def execute(case, original):
        calls = Counter()
        with ExitStack() as stack:
            with patch.object(case, "ioclass", lambda initial: BinaryStream(binary, stack, calls, initial)):
                original()
        if calls != {"BytesIO": 3, "getvalue": 5, "read": 1, "close": 1}:
            raise AssertionError(dict(calls))
        return {"roles": {"body": "cpython", "assertions": "cpython", "operation": "imre", "fixture": "mixed"},
                "evidence": {"native_calls": dict(calls), "method_sha256": digest(method_bytes(source, OWNER, "test_getvalue"))}}
    return execute


def digest(data):
    return hashlib.sha256(data).hexdigest()


def subdir(root, name):
    directory = root / name
    directory.mkdir()
    return directory


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "cpython-source", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        raise ValueError("the original test and fixtures require CPython 3.14.5")
    verify_files(HERE.parents[1] / "doctest" / "upstream", {SOURCE: BLOB})
    verify_files(args.cpython_source, {SOURCE: BLOB})
    sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    verify_interpreter_tests({"files": {SOURCE: BLOB}})
    discovered = [case.id() for case in cases(load_suite(MODULE))]
    if discovered.count(CASE) != 1:
        raise ValueError("the original ID was not discovered exactly once")
    report = {"cpython": "5607950ef232dad16d75c0cf53101d9649d89115", "source": SOURCE, "blob": BLOB,
              "case": CASE, "discovered_in_module": len(discovered), "observations": {}}
    source = args.cpython_source / SOURCE
    control = HERE / "snapshots.py"
    reference = subprocess.run([sys.executable, "-B", control], capture_output=True, check=True, timeout=30)
    for level in (0, 3):
        toolchain = Toolchain(args.imre, args.runtime, args.clang, level)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            binary = toolchain.build(control.read_bytes(), subdir(root, "snapshots"))
            actual = subprocess.run([binary], capture_output=True, check=True, timeout=30)
            if (reference.stdout, reference.stderr) != (actual.stdout, actual.stderr):
                raise AssertionError((reference.stdout, actual.stdout, reference.stderr, actual.stderr))
            record = {"snapshots": {"stdout_sha256": digest(actual.stdout), "stdout_bytes": len(actual.stdout)}}
            try:
                toolchain.build((HERE / "ordinary_file.py").read_bytes(), subdir(root, "ordinary-file"))
            except NativeCompileError as error:
                diagnostic = str(error).replace(tmp, "<tmp>").strip()
                if ":6:12: Imre: supported file methods" not in diagnostic and "getvalue" not in diagnostic:
                    raise
                record["ordinary_binary_file"] = diagnostic
            else:
                raise AssertionError("ordinary binary files exposed getvalue")
            try:
                execute = native_method(toolchain, subdir(root, "method"), source, OWNER, CASE,
                                        FIXTURE, operation=("imre", "native io.BytesIO"))
            except NativeCompileError as error:
                record["complete_native_compile_blocker"] = str(error).replace(tmp, "<tmp>").strip()
                driver = toolchain.build((HERE / "driver.py").read_bytes(), subdir(root, "driver"))
                execute = host_executor(driver, source)
            observed, failures, skips = run_in_fixtures(load_suite(MODULE), {CASE: execute})
            if failures or skips or len(observed.get(CASE, [])) != 1:
                raise AssertionError((failures, skips, observed))
            value, = observed[CASE]
            record["original_method"] = value.get("observation", value)
            report["observations"][f"O{level}"] = record
            print(f"O{level}: {CASE}: {record['original_method']['roles']}", flush=True)
    args.result.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
