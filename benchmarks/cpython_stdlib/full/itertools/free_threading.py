"""Run the original batched threading companion on a free-threaded CPython 3.14.5."""

import argparse
import json
from pathlib import Path
import subprocess
import sys
import sysconfig
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from coverage import HERE, blocked
from suite_evidence import build_report, write_reports
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import NativeCompileError, Toolchain

MODULE = "test.test_free_threading.test_itertools_batched"
SOURCE = "Lib/test/test_free_threading/test_itertools_batched.py"


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + arg, type=Path, required=True)
    p.add_argument("--write-reference", action="store_true")
    p.add_argument("--gil-python", type=Path, help="also verify the original pre-discovery package guard")
    args = p.parse_args()
    if (sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5)
            or not sysconfig.get_config_var("Py_GIL_DISABLED") or sys._is_gil_enabled()):
        p.error("use a test-enabled, free-threaded CPython 3.14.5 with the GIL disabled")
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    suite = load_suite(MODULE)
    discovery = [(c.id(), f"{type(c).__module__}.{type(c).__qualname__} (free-threaded)")
                 for c in cases(suite)]
    result = unittest.TestResult()
    suite.run(result)
    if len(discovery) != 1 or result.testsRun != 1 or result.skipped or not result.wasSuccessful():
        raise ValueError(f"companion discovery or result changed: {discovery}, {result.skipped}, {result.errors}, {result.failures}")
    reference = {"module": MODULE, "cpython_commit": manifest["commit"], "platform": sys.platform,
                 "configuration": "free-threaded CPython, GIL disabled", "denominator": len(discovery),
                 "reference": {name: "pass" for name, _ in discovery}}
    ref = HERE / "reference-free-threading-linux.json"
    if args.write_reference:
        ref.write_text(json.dumps(reference, indent=2) + "\n")
    elif reference != json.loads(ref.read_text()):
        raise ValueError("unchanged free-threaded reference differs")
    if args.gil_python:
        guard = (
            "import importlib, sys, sysconfig, unittest\n"
            "assert sys.version_info[:3] == (3, 14, 5) and not sysconfig.get_config_var('Py_GIL_DISABLED')\n"
            f"try: importlib.import_module({MODULE!r})\n"
            "except unittest.SkipTest as exc: print(str(exc))\n"
            "else: raise AssertionError('free-threading package guard did not run')\n")
        observed = subprocess.run([args.gil_python, "-B", "-c", guard],
                                  capture_output=True, text=True, check=True)
        if observed.stdout != "GIL enabled\n" or observed.stderr:
            raise ValueError(f"unexpected package guard: {observed}")
    reports = []
    for level in (0, 3):
        with tempfile.TemporaryDirectory() as work:
            directory = Path(work)
            tools = Toolchain(args.imre, args.runtime, args.clang, level)
            # Keep imports, the reap_threads decorator, all workers and Barrier,
            # start_threads and the original unittest entry point together.
            try:
                tools.build((HERE / "upstream" / SOURCE).read_bytes(), directory)
            except NativeCompileError as exc:
                observation = blocked(exc, directory)
            else:
                raise ValueError("the whole threading companion compiled; verify its full native unittest result")
            observation["evidence"].update({
                "source": SOURCE, "source_blob": manifest["files"][SOURCE],
                "reference_configuration": reference["configuration"],
                "gil_enabled_configuration": {"discovered_before_skip": 0, "reason": "GIL enabled",
                    "source": "Lib/test/test_free_threading/__init__.py:8"}})
            reports.append(build_report(reference, level, discovery, {(discovery[0][0], 1): observation}))
    write_reports(args.result, reports, reference)
    print(f"{MODULE}: one unchanged reference pass, {reports[0]['counts']} native at each level")


if __name__ == "__main__":
    main()
