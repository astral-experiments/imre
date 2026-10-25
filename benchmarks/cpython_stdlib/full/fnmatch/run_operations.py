"""Run the unchanged CPython suite with each fnmatch operation supplied by an Imre binary."""
import argparse
import ast
from collections import Counter
import fnmatch
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain

FUNCTIONS = ("fnmatch", "fnmatchcase", "translate", "filter", "filterfalse", "_translate")


class Trace:
    def __init__(self):
        self.original = {name: getattr(fnmatch, name) for name in FUNCTIONS}
        self.case = None
        self.depth = 0
        self.calls = {}
        self.by_case = {}
        self.replay = None

    def wrapper(self, name):
        def call(*args):
            if self.depth or self.case is None:
                return self.original[name](*args)
            key = name, repr(args)
            self.by_case.setdefault(self.case, []).append(key)
            if self.replay is not None:
                ok, value = self.replay[key]
                if not ok:
                    raise TypeError(value)
                return value
            self.depth += 1
            try:
                value = self.original[name](*args)
            except TypeError as exc:
                self.calls[key] = args, (False, str(exc))
                raise
            finally:
                self.depth -= 1
            self.calls[key] = args, (True, value)
            return value
        return call

    def run(self):
        trace = self
        class Cases(unittest.TestResult):
            def startTest(self, test):
                super().startTest(test)
                trace.case = test.id()
            def stopTest(self, test):
                trace.case = None
                super().stopTest(test)
        result = Cases()
        load_suite("test.test_fnmatch").run(result)
        if not result.wasSuccessful() or result.skipped:
            raise ValueError((result.errors, result.failures, result.skipped))
        return result.testsRun


def annotation(v):
    if isinstance(v, list):
        return "list[" + (annotation(v[0]) if v else "str") + "]"
    return type(v).__name__


def driver(calls):
    functions = {}
    body = []
    for (name, _), (args, _) in calls.items():
        key = name, tuple(map(annotation, args))
        handler = functions.setdefault(key, f"emit_{len(functions)}")
        body.append(handler + "(" + ", ".join(map(repr, args)) + ")")
    source = ["import fnmatch"]
    for (name, types), handler in functions.items():
        params = [f"p{i}" for i in range(len(types))]
        source.append(f"def {handler}(" + ", ".join(f"{p}: {ty}" for p, ty in zip(params, types)) + ") -> None:")
        source.append("    try:")
        call = f"fnmatch.{name}(" + ", ".join(params) + ")"
        if name == "_translate":
            source.extend(["        parts, stars = " + call, "        print('(' + repr(parts) + ', ' + repr(stars) + ')')"])
        else:
            source.append("        print(repr(" + call + "))")
        source += ["    except TypeError as exc:", "        print('!TypeError: ' + str(exc))"]
    # Keep individual function sizes independent of the number of upstream inputs.
    for i in range(0, len(body), 100):
        source += [f"def chunk_{i}() -> None:", *["    " + line for line in body[i:i+100]]]
    source += [f"chunk_{i}()" for i in range(0, len(body), 100)]
    return ("\n".join(source) + "\n").encode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for opt in ("imre", "runtime", "clang"):
        parser.add_argument("--"+opt, type=Path, required=True)
    parser.add_argument("--result", type=Path, required=True)
    parser.add_argument("--opt-levels", type=int, nargs="+", default=[0, 3])
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or sys.implementation.name != "cpython":
        parser.error("use a test-enabled CPython 3.14.5")
    manifest = json.loads((HERE/"UPSTREAM.json").read_bytes())
    verify_files(HERE/"upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    reference = json.loads((HERE/"reference-linux.json").read_bytes())
    trace = Trace()
    with patch.multiple(fnmatch, **{name:trace.wrapper(name) for name in FUNCTIONS}):
        # Patch before import: check_match captures fnmatch as a default argument.
        if "test.test_fnmatch" in sys.modules:
            raise RuntimeError("the unchanged test module must be imported after operation routing")
        count = trace.run()
        expected_calls = trace.by_case
        if set(expected_calls) != set(reference["reference"]) or count != reference["denominator"]:
            raise ValueError("observed test IDs differ from the pinned reference")
        code = driver(trace.calls)
        reports = {}
        for level in args.opt_levels:
            with tempfile.TemporaryDirectory() as tmp:
                binary = Toolchain(args.imre, args.runtime, args.clang, level).build(code, Path(tmp))
                process = subprocess.run([binary], check=True, capture_output=True, text=True, timeout=120)
            if process.stderr:
                raise ValueError(process.stderr)
            native = {}
            for (key, (_, expected)), row in zip(trace.calls.items(), process.stdout.splitlines(), strict=True):
                actual = (False, row.removeprefix("!TypeError: ")) if row.startswith("!TypeError: ") else (True, ast.literal_eval(row))
                if actual != expected:
                    raise ValueError(f"{key}: {actual!r} differs from reference {expected!r}")
                native[key] = actual
            trace.by_case = {}
            trace.replay = native
            if trace.run() != count or trace.by_case != expected_calls:
                raise ValueError("native replay changed the original assertion or operation sequence")
            reports[str(level)] = {name: dict(Counter(k[0] for k in keys)) for name,keys in expected_calls.items()}
            print(f"O{level}: {count} unchanged host-driven methods, {len(native)} distinct native requests, "
                  f"{sum(map(len, expected_calls.values()))} original calls")
    args.result.write_text(json.dumps({"cpython_commit":manifest["commit"], "execution":"CPython test bodies, fixtures and assertions; Imre fnmatch operations",
        "distinct_native_requests":len(trace.calls), "levels":reports}, indent=2)+"\n")


if __name__ == "__main__":
    main()
