"""Run both unchanged CPython stat fixtures with native mode functions and constants."""

import argparse
import ast
from collections import Counter
from contextlib import chdir, ExitStack
from functools import cache
import importlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent.parent))
from suite_evidence import build_report, roles, write_reports
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain
from generate_module import generate

FUNCTIONS = {"S_IFMT", "S_IMODE", "filemode"} | {
    "S_IS" + name
    for name in (
        "DIR",
        "REG",
        "FIFO",
        "LNK",
        "BLK",
        "CHR",
        "SOCK",
        "DOOR",
        "PORT",
        "WHT",
    )
}


def source_for(import_name, constants):
    lines = [
        f"import {import_name} as target",
        "import sys",
        "def dispatch(op: str, n: int) -> None:",
    ]
    for name in sorted(constants):
        lines.extend(
            [
                f"    if op == {name!r}:",
                f"        print(repr(target.{name}))",
                "        return",
            ]
        )
    for name in sorted(FUNCTIONS):
        lines.extend(
            [
                f"    if op == {'bound-' + name!r}:",
                f"        bound_{name} = target.{name}",
                f"        print(repr(bound_{name}(n)))",
                "        return",
                f"    if op == {name!r}:",
                "        try:",
                f"            print(repr(target.{name}(n)))",
                "        except OverflowError as exc:",
                "            print('!overflow:' + str(exc))",
                "        return",
            ]
        )
    lines.extend(
        [
            "    raise ValueError('unknown request')",
            "dispatch(sys.argv[1], int(sys.argv[2]))",
        ]
    )
    return ("\n".join(lines) + "\n").encode()


class NativeStat:
    def __init__(self, base, pure, tool, work, result):
        self.result = result
        self.module = "stat_fallback" if pure else "stat"
        self.constants = {
            n
            for n in dir(base)
            if not n.startswith("_") and type(getattr(base, n)) is int
        }
        self.work = work
        self.tool = tool
        work.mkdir()
        if pure:
            (work / "stat_fallback.py").write_text(generate(pure=True))
        self.binary = tool.build(source_for(self.module, self.constants), work)

    @cache
    def query(self, op, mode=0):
        run = subprocess.run(
            [self.binary, op, str(mode)], check=True, text=True, capture_output=True
        )
        if run.stderr:
            raise ValueError(run.stderr)
        row = run.stdout.removesuffix("\n")
        if row.startswith("!overflow:"):
            return False, row.removeprefix("!overflow:")
        return True, ast.literal_eval(row)

    @cache
    def confirm_missing(self, name):
        if not name.isidentifier():
            raise ValueError(name)
        probe = self.work / "missing.py"
        probe.write_text(f"from {self.module} import {name}\nprint({name})\n")
        run = subprocess.run(
            [self.tool.imre, "check", probe], text=True, capture_output=True
        )
        if (
            not run.returncode
            or name not in run.stderr
            or "static binding" not in run.stderr
        ):
            raise AssertionError((name, run.stderr))

    def __getattr__(self, name):
        self.result.hit("get:" + name)
        if name in self.constants:
            ok, result = self.query(name)
            if not ok or type(result) is not int:
                raise AssertionError((name, result))
            return result
        if name in FUNCTIONS:
            if self.query("bound-" + name) != self.query(name):
                raise AssertionError(
                    f"{self.module}.{name} did not resolve to a native callable"
                )

            def invoke(mode):
                self.result.hit("call:" + name)
                ok, value = self.query(name, mode)
                if not ok:
                    raise OverflowError(value)
                return value

            return invoke
        self.confirm_missing(name)
        raise AttributeError(name)


class Results(unittest.TestResult):
    def __init__(self):
        super().__init__()
        self.current = None
        self.calls = {}

    def startTest(self, case):
        super().startTest(case)
        self.current = case.id()
        self.calls[self.current] = Counter()

    def stopTest(self, case):
        self.current = None
        super().stopTest(case)

    def hit(self, op):
        if self.current is not None:
            self.calls[self.current][op] += 1


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    for opt in ("imre", "runtime", "clang", "result"):
        ap.add_argument("--" + opt, type=Path, required=True)
    ap.add_argument("--cpython-source", type=Path)
    a = ap.parse_args()
    if a.cpython_source:
        sys.path.insert(0, str(a.cpython_source.resolve() / "Lib"))
    if sys.version_info[:3] != (3, 14, 5):
        ap.error("use CPython 3.14.5")
    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    module = importlib.import_module(manifest["module"])
    pin = manifest["commit"]
    discovered = [
        (case.id(), type(case).__name__)
        for case in cases(load_suite(manifest["module"]))
    ]
    with tempfile.TemporaryDirectory() as refdir, chdir(refdir):
        refresult = unittest.TestResult()
        load_suite(manifest["module"]).run(refresult)
    if not refresult.wasSuccessful():
        raise AssertionError((refresult.errors, refresult.failures))
    skipped = {c.id(): reason for c, reason in refresult.skipped}
    ref = {
        "module": manifest["module"],
        "cpython_commit": pin,
        "platform": sys.platform,
        "denominator": len(discovered),
        "reference": {
            name: {"skipped": skipped[name]} if name in skipped else "pass"
            for name, _ in discovered
        },
    }
    if sys.platform == "linux" and ref != json.loads(
        (HERE / "reference-linux.json").read_bytes()
    ):
        raise ValueError("reference diverged from the pinned Linux ledger")
    reports = []
    for level in (0, 3):
        tool = Toolchain(
            a.imre.resolve(), a.runtime.resolve(), a.clang.resolve(), level
        )
        result = Results()
        with tempfile.TemporaryDirectory() as tmp, ExitStack() as stack:
            stack.enter_context(chdir(tmp))
            for attr, cls, pure in (
                ("c_stat", module.TestFilemodeCStat, False),
                ("py_stat", module.TestFilemodePyStat, True),
            ):
                base = getattr(module, attr)
                native = NativeStat(base, pure, tool, Path(tmp) / attr, result)
                stack.enter_context(patch.object(module, attr, native))
                stack.enter_context(patch.object(cls, "statmod", native))
                # Check the two distinct mode domains, including C converter errors.
                for fn in ("S_IMODE", "S_ISDIR", "S_ISWHT", "filemode"):
                    for n in (
                        -1,
                        0,
                        0o160000,
                        2**16,
                        2**32 - 1,
                        2**32,
                        2**64 - 1,
                        2**64,
                        2**140,
                    ):
                        try:
                            expected = (True, getattr(base, fn)(n))
                        except OverflowError as exc:
                            expected = (False, str(exc))
                        actual = native.query(fn, n)
                        if (
                            type(actual[1]) is not type(expected[1])
                            or actual != expected
                        ):
                            raise ValueError((attr, fn, n, actual, expected))
            load_suite(manifest["module"]).run(result)
        if (
            not result.wasSuccessful()
            or result.testsRun != len(discovered)
            or {c.id(): s for c, s in result.skipped} != skipped
        ):
            raise AssertionError((result.errors, result.failures, result.skipped))
        observations = {}
        exceptions = {}
        for name, variant in discovered:
            if name in skipped:
                exceptions[name, 1, level] = {
                    "kind": "upstream-skip",
                    "reason": skipped[name],
                    "condition": "original decorator on " + name.rsplit(".", 1)[1],
                    "source": "Lib/test/test_stat.py",
                    "configuration": "CPython 3.14.5 with POSIX filesystem fixtures",
                    "platform": sys.platform,
                    "fixture_variant": variant,
                }
                continue
            if not result.calls[name]:
                raise AssertionError(f"{name} passed without a native stat operation")
            observations[name, 1] = {
                "outcome": "pass",
                "roles": roles(
                    body="cpython",
                    assertions="cpython",
                    operation="imre",
                    fixture="cpython",
                ),
                "evidence": {
                    "opt_level": level,
                    "operation": "Compiled stat mode functions and checked exports, called with values from real filesystem fixtures.",
                    "fixture": variant
                    + "; original CPython unittest body, assertions, filesystem setup and cleanup.",
                    "native_requests": dict(sorted(result.calls[name].items())),
                },
            }
        report = build_report(
            ref, level, discovered, observations, exceptions=exceptions
        )
        print(f"O{level}: {report['counts']}", flush=True)
        reports.append(report)
    write_reports(a.result.resolve(), reports, ref)


if __name__ == "__main__":
    main()
