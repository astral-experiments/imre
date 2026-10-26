"""Feed native shlex results back to every applicable unchanged CPython test."""
import argparse
import ast
from collections import Counter, defaultdict
from contextlib import ExitStack
from functools import cache
from inspect import signature
import json
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import roles, write_reports
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_evidence


def arguments(name, params):
    if name == "join":
        return ("join", *params["split_command"])
    value = params["s"]
    if name == "split":
        if value is None:
            return ("split-none",)
        return ("split", value, str(int(params["comments"])), str(int(params["posix"])))
    kind = type(value).__name__
    if value is None:
        return ("quote-none",)
    if kind == "bytes":
        return ("quote-bytes", *map(str, value))
    return ("quote-" + kind, str(value))


def run(tools, work, binary, reference, module):
    level = tools.level
    suite = load_suite(module)
    counts = defaultdict(Counter)

    class Results(unittest.TestResult):
        current = None

        def startTest(self, test):
            super().startTest(test)
            self.current = test.id()

        def stopTest(self, test):
            self.current = None
            super().stopTest(test)

    @cache
    def native(request):
        out = subprocess.run([binary, *request], check=True, capture_output=True, text=True, timeout=30)
        if out.stderr:
            raise AssertionError(out.stderr)
        return out.stdout.removesuffix("\n")

    results = Results()
    depth = 0

    def wrapper(name, original):
        sig = signature(original)

        def call(*args, **kwargs):
            nonlocal depth
            if depth or results.current is None:
                return original(*args, **kwargs)
            bound = sig.bind(*args, **kwargs)
            bound.apply_defaults()
            counts[results.current][name] += 1
            depth += 1
            try:
                expected = original(*args, **kwargs)
            except (TypeError, ValueError) as exc:
                expected = exc
            finally:
                depth -= 1
            raw = native(arguments(name, bound.arguments))
            if isinstance(expected, Exception):
                if raw != f"!{type(expected).__name__}: {expected}":
                    raise AssertionError((bound.arguments, expected, raw))
                raise type(expected)(raw.partition(": ")[2])
            actual = ast.literal_eval(raw)
            if actual != expected or type(actual) is not type(expected):
                raise AssertionError((bound.arguments, expected, actual))
            return actual
        return call

    with ExitStack() as stack:
        for name in ("quote", "join", "split"):
            stack.enter_context(patch.object(shlex, name, wrapper(name, getattr(shlex, name))))
        suite.run(results)
    if not results.wasSuccessful() or results.skipped or results.testsRun != reference["denominator"]:
        raise AssertionError((results.errors, results.failures, results.skipped, results.testsRun))
    observed = {
        (name, 1): {
            "outcome": "pass",
            "roles": roles(fixture="cpython", body="cpython", assertions="cpython", operation="imre"),
            "evidence": {"fixture": "Original ShlexTest.setUp and unittest lifecycle",
                         "operation": ", ".join(sorted(ops)), "calls_by_name": dict(ops),
                         "native_calls": sum(ops.values()), "opt_level": level},
        }
        for name, ops in counts.items()
    }
    print(f"O{level}: {len(observed)}/{results.testsRun} methods exercised native shlex calls; "
          f"{sum(sum(c.values()) for c in counts.values())} calls, {native.cache_info().misses} distinct")
    # These bodies don't read instance data; their real upstream setUp still runs.
    executions = {}
    source = HERE / "upstream/Lib/test/test_shlex.py"
    fixture = "import shlex\nimport string\nimport unittest\nclass NativeCase(unittest.TestCase):\n"
    for method, operation in (("testSplitNone", "shlex.split"), ("testQuote", "shlex.quote")):
        case_id = f"{module}.ShlexTest.{method}"
        directory = work / method
        directory.mkdir()
        executions[case_id] = native_method(tools, directory, source, "ShlexTest", case_id, fixture,
                                            operation=("imre", operation))
    report = run_evidence(load_suite(module), executions, reference, level,
                          preflight={key: value for key, value in observed.items() if key[0] not in executions})
    for row in report["records"]:
        if row["id"] in executions:
            if row["classification"] != "native-body-host-fixture":
                raise AssertionError(f"{row['id']}: {row['outcome']}: {row.get('reason')}")
            row["evidence"]["host_replay_calls_by_name"] = dict(counts[row["id"]])
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path, help="pinned CPython tree exposing Lib/test")
    args = parser.parse_args()
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    if sys.implementation.name != "cpython" or manifest["tag"] != "v" + ".".join(map(str, sys.version_info[:3])):
        raise ValueError("use the pinned CPython")
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    verify_files(Path(shlex.__file__).parent, {"shlex.py": manifest["files"]["Lib/shlex.py"]})
    reference = json.loads((HERE / "reference-linux.json").read_bytes())
    edges = HERE / "edge_cases.py"
    expected_edges = subprocess.check_output([sys.executable, "-I", edges], timeout=60)
    reports = []
    for level in (0, 3):
        with tempfile.TemporaryDirectory() as tmp:
            tools = Toolchain(args.imre, args.runtime, args.clang, level)
            binary = tools.build((HERE / "native_calls.py").read_bytes(), Path(tmp))
            reports.append(run(tools, Path(tmp), binary, reference, manifest["module"]))
            binary = tools.build(edges.read_bytes(), Path(tmp))
            actual = subprocess.run([binary], check=True, capture_output=True, timeout=60)
            if actual.stderr or actual.stdout != expected_edges:
                raise AssertionError(f"O{level}: shell word or iteration-order differential failed")
            print(f"O{level}: {len(expected_edges.splitlines())} edge observations match CPython")
    write_reports(args.result, reports, reference)


if __name__ == "__main__":
    main()
