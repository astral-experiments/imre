"""Run unchanged test_time methods on native local time and inventory the full pinned suite."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import roles, write_reports
from suite_inventory import (
    discovery_resources,
    load_suite,
    verify_files,
    verify_interpreter_tests,
)
from suite_native import NativeCompileError, Toolchain, method_bytes, run_evidence

OWNER = "TimeTestCase"
SOURCE = HERE / "upstream/Lib/test/test_time.py"
METHODS = (
    "test_mktime",
    "test_epoch",
    "test_gmtime_without_arg",
    "test_localtime_without_arg",
)
SKIPS = {
    "test_mktime_error": (
        510,
        "The original libc_ver guard disables extreme mktime inputs on glibc.",
    ),
    "test_monotonic_settime": (
        598,
        "The original test catches permission denied when setting CLOCK_REALTIME.",
    ),
    "test_clock_functions": (1162, "The original weak-linking class runs on Darwin."),
}
HEADER = b"""from time import struct_time
import unittest
class Case(unittest.TestCase):
    t: float
    def __init__(self):
        super().__init__()
        self.t = 0.0
"""


def source(name, wrong=False):
    if name == "test_epoch":
        assertion = b"""    def assertEqual(self, first: tuple[int,int,int,int,int,int], second: tuple[int,int,int,int,int,int], msg: struct_time) -> None:
        self.assertTrue(first == second)
"""
    elif name == "test_mktime":
        assertion = b"""    def assertEqual(self, first: float, second: int) -> None:
        self.assertTrue(first == second)
"""
    else:
        assertion = b"""    def assertAlmostEqual(self, first: float, second: float, delta: float) -> None:
        self.assertTrue(abs(first - second) <= delta)
"""
    imports = b"import clock_control as time\n" if wrong else b"import time\n"
    original = method_bytes(SOURCE, OWNER, name)
    return (
        imports
        + HEADER
        + assertion
        + method_bytes(SOURCE, OWNER, "setUp")
        + original
        + f"\ncase = Case()\ncase.setUp()\ncase.{name}()\nprint('completed')\n".encode()
    )


def control(name):
    module = b"from time import time, struct_time\nimport time as original\n"
    if name == "test_mktime":
        return (
            module
            + b"from time import localtime\ndef mktime(t: struct_time) -> float:\n    return original.mktime(t) + 1.0\n"
        )
    if name == "test_epoch":
        return (
            module
            + b"def gmtime(t: int) -> struct_time:\n    return original.gmtime(t + 86400)\n"
        )
    fn = "gmtime" if name == "test_gmtime_without_arg" else "localtime"
    return (
        module
        + (
            f"from time import mktime\ndef {fn}(t: int | None = 0) -> struct_time:\n"
            f"    return original.{fn}(1 if t is None else 0)\n"
        ).encode()
    )


def prepare(tool, work, name):
    key = f"test.test_time.{OWNER}.{name}"
    body = method_bytes(SOURCE, OWNER, name)
    if name == "test_conversions":
        try:
            tool.build(source(name), work)
        except NativeCompileError as error:
            diagnostic = str(error).rsplit("Imre: ", 1)
            if len(diagnostic) != 2 or "name is not a local value" not in diagnostic[1]:
                raise
            return key, {
                "outcome": "blocked",
                "reason": diagnostic[1].strip(),
                "roles": roles(
                    body="unobserved",
                    assertions="unobserved",
                    operation="unobserved",
                    fixture="unobserved",
                ),
                "evidence": {
                    "stage": "compile",
                    "source": "Lib/test/test_time.py:156",
                    "method_sha256": hashlib.sha256(body).hexdigest(),
                },
            }
        raise AssertionError("review the newly compiled original conversion method")

    (work / "clock_control.py").write_bytes(control(name))
    wrong = tool.build(source(name, wrong=True), work)
    result = subprocess.run([wrong], capture_output=True, timeout=30)
    assert result.returncode and b"AssertionError" in result.stderr, (key, result)
    binary = tool.build(source(name), work)

    def execute(case, original_method):
        assert case.id() == key and original_method.__name__ == name
        result = subprocess.run([binary], capture_output=True, timeout=30)
        assert (result.returncode, result.stdout, result.stderr) == (
            0,
            b"completed\n",
            b"",
        ), (key, result)
        return {
            "roles": roles(
                body="imre", assertions="imre", operation="imre", fixture="mixed"
            ),
            "evidence": {
                "method_sha256": hashlib.sha256(body).hexdigest(),
                "setup_sha256": hashlib.sha256(
                    method_bytes(SOURCE, OWNER, "setUp")
                ).hexdigest(),
                "operation": "The unchanged method and setUp call native time functions.",
                "fixture": "CPython runs suite discovery and outer hooks. The native class supplies typed numeric or tuple assertions through TestCase.assertTrue.",
                "negative_control": "A displaced native timestamp fails the unchanged method.",
            },
        }

    return key, execute


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("compiler", "runtime", "clang", "result"):
        parser.add_argument("--" + arg, type=Path, required=True)
    parser.add_argument("--reachable-only", action="store_true")
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        parser.error("use pinned CPython 3.14.5")
    spec = json.loads((HERE / "UPSTREAM.json").read_text())
    ref = json.loads((HERE / "reference-linux.json").read_text())
    verify_files(HERE / "upstream", spec["files"])
    verify_interpreter_tests(spec)
    reports = []
    with discovery_resources(spec), tempfile.TemporaryDirectory() as tmp:
        for level in (0, 3):
            executions, blocked, exceptions = {}, {}, {}
            for name in (*METHODS, "test_conversions"):
                work = Path(tmp) / str(level) / name
                work.mkdir(parents=True)
                key, item = prepare(
                    Toolchain(
                        args.compiler,
                        args.runtime,
                        args.clang,
                        level,
                        args.reachable_only,
                        module_root=work,
                    ),
                    work,
                    name,
                )
                (executions if callable(item) else blocked)[key] = item
            for key, outcome in ref["reference"].items():
                if isinstance(outcome, dict):
                    line, condition = SKIPS[key.rsplit(".", 1)[1]]
                    exceptions[(key, 1, level)] = {
                        "kind": "upstream-skip",
                        "platform": "linux",
                        "configuration": "CPython 3.14.5 default regrtest resources",
                        "fixture_variant": key.rsplit(".", 1)[0],
                        "reason": outcome["skipped"],
                        "condition": condition,
                        "source": f"Lib/test/test_time.py:{line}",
                    }
            observations = {(key, 1): value for key, value in blocked.items()}
            reports.append(
                run_evidence(
                    load_suite(spec["module"]),
                    executions,
                    ref,
                    level,
                    preflight=observations,
                    exceptions=exceptions,
                )
            )
            print(f"O{level} {reports[-1]['counts']}", flush=True)
    write_reports(args.result, reports, ref)


if __name__ == "__main__":
    main()
