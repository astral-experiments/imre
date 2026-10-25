"""Run pinned SHA-256 vectors and incremental hash tests against native Imre."""
import argparse
import ast
from collections import Counter
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from upstream import verify_upstream

DRIVER = """import hashlib
import sys

with open(sys.argv[1], "rb") as stream:
    h = hashlib.sha256(stream.read())
print(repr((h.hexdigest(), h.digest(), h.digest_size)))
"""


NAMES = r"""import hashlib
from hashlib import algorithms_available
print("sha256" in algorithms_available)
algorithms_available.add("imre-control-unknown")
print("imre-control-unknown" in hashlib.algorithms_available)
for name in ["sha256", "Sha256", "SHA-256", "sha2-256", "2.16.840.1.101.3.4.2.1", "", "spam", "sha256\x00", "imre-control-unknown"]:
    try:
        state = hashlib.new(name=name, data=b"abc", usedforsecurity=False)
        copied = state.copy()
        state.update(b"d")
        print(repr((state.name, copied.hexdigest(), state.hexdigest())))
    except ValueError as error:
        print(repr(str(error)))
"""


def run_case(suite, name, program, payload):
    case = suite.HashLibTestCase(name)
    original = suite.hashlib.sha256
    constructors = case.constructors_to_test["sha256"]
    constructors.remove(original)
    host_constructors = {f"{ctor.__module__}.{ctor.__name__}": mock.Mock(wraps=ctor) for ctor in constructors}
    constructors.clear()
    constructors.update(host_constructors.values())
    host_calls = Counter()
    checked = Counter()
    lengths = []

    class NativeVector:
        def __init__(self, data):
            payload.write_bytes(data)
            result = subprocess.run([program, payload], check=True, capture_output=True, timeout=30)
            if result.stderr:
                raise AssertionError(result.stderr)
            self.hex, self.raw, self.size = ast.literal_eval(result.stdout.decode("ascii"))
            lengths.append(len(data))

        def hexdigest(self):
            checked["hexdigest"] += 1
            return self.hex

        def digest(self):
            checked["digest"] += 1
            return self.raw

        @property
        def digest_size(self):
            checked["digest_size"] += 1
            return self.size

    def direct(*args):
        if args:  # The vector calls the public constructor with its input bytes.
            return NativeVector(*args)
        # The file_digest helper requests an empty, mutable host constructor.
        host_calls["empty constructor for file_digest"] += 1
        return original()

    constructors.add(direct)
    with mock.patch.object(suite.hashlib, "file_digest", wraps=suite.hashlib.file_digest) as files:
        result = unittest.TestResult()
        case.run(result)
    host_calls["file_digest"] = files.call_count
    passed = (result.wasSuccessful() and result.testsRun == 1 and not result.skipped and len(lengths) == 1
              and checked == {"hexdigest": 1, "digest": 1, "digest_size": 1})
    return case.id(), {
        "status": "mixed" if passed else "fail",
        "native_one_shot_input_bytes": lengths,
        "native_results_checked": dict(checked),
        "host_operations": dict(host_calls),
        "host_reference_constructors": {name: fn.call_count for name, fn in sorted(host_constructors.items())},
        "diagnostics": [reason for _, reason in result.failures + result.errors + result.skipped],
    }


def run_native_methods(suite, work, build):
    methods = {"test_sha256_gil": 2, "test_sha256_update_over_4gb": 1, "test_new_upper_to_lower": 1}
    source = Path(suite.__file__).read_bytes()
    cls = next(n for n in ast.parse(source).body if isinstance(n, ast.ClassDef) and n.name == "HashLibTestCase")
    lines = source.splitlines(keepends=True)
    bodies = {n.name: b"".join(lines[n.lineno - 1:n.end_lineno]) for n in cls.body
              if isinstance(n, ast.FunctionDef) and n.name in methods}
    # unittest opts the large test out without the CPU resource. This benchmark
    # opts it in and copies the original method without its scheduling decorator.
    # The private CPython module constants in the other fixture are host input.
    threshold = suite.hashlib_helper.find_gil_minsize(["_sha2", "_hashlib"])
    (work / "hashlib_helper.py").write_text(f"""def find_gil_minsize(modules: list[str]) -> int:
    if len(modules) != 2 or modules[0] != "_sha2" or modules[1] != "_hashlib":
        raise ValueError("unexpected SHA-256 threshold fixture")
    return {threshold}
""")
    adapter = b"""import hashlib
import hashlib_helper
import sys

class Case:
    checks: int

    def __init__(self) -> None:
        self.checks = 0

    def assertEqual(self, actual: str, expected: str) -> None:
        if actual != expected:
            raise ValueError(f"expected {expected}, got {actual}")
        self.checks += 1

"""
    dispatch = "case = Case()\n" + "".join(f'if sys.argv[1] == "{name}":\n    case.{name}()\n' for name in methods)
    driver = work / "methods.py"
    driver.write_bytes(adapter + b"\n".join(bodies[name] for name in methods) + b"\n" +
                       (dispatch + "print(case.checks)\n").encode())
    binary = build(driver)
    tests = {}
    for name, checks in methods.items():
        run = subprocess.run([binary, name], cwd=work, text=True, capture_output=True, timeout=120)
        passed = not run.returncode and not run.stderr and run.stdout == f"{checks}\n"
        row = {"status": "pass" if passed else "fail", "native_assertions": checks if passed else None,
               "method_sha256": hashlib.sha256(bodies[name]).hexdigest()}
        if name == "test_sha256_gil":
            row["host_fixture_gil_minsize"] = threshold
        if not passed:
            row["diagnostics"] = [run.stderr or repr(run.stdout)]
        tests[suite.HashLibTestCase(name).id()] = row
    return tests


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    parser.add_argument("--opt-level", choices=("0", "3"), default="3")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("the original hashlib fixtures require CPython 3.14.5")
    commit = verify_upstream()
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    vendored = HERE / "upstream/Lib/test"
    test.__path__.insert(0, str(vendored))
    from test import test_hashlib
    if Path(test_hashlib.__file__).resolve() != vendored / "test_hashlib.py":
        raise RuntimeError("the selected methods must come from the verified vendored file")

    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        def build(source):
            program = source.with_suffix("")
            subprocess.run([args.imre.resolve(), "build", source, "-o", program, "--no-cache",
                            "--opt-level", args.opt_level, "--clang", args.clang.resolve(), "--runtime", args.runtime.resolve()],
                           cwd=work, check=True, timeout=120)
            return program

        source = work / "hash.py"
        source.write_text(DRIVER)
        program = build(source)
        cases = dict(run_case(test_hashlib, f"test_case_sha256_{n}", program, work / "data") for n in range(4))
        controls = work / "names.py"
        controls.write_text(NAMES)
        binary = build(controls)
        def run(command):
            return subprocess.run(command, capture_output=True, timeout=30)
        reference, actual = run([sys.executable, controls]), run([binary])
        assert reference.returncode == 0 and not reference.stderr, reference
        assert (actual.returncode, actual.stdout, actual.stderr) == (0, reference.stdout, b""), actual
        native = run_native_methods(test_hashlib, work, build)
        cases.update(native)
    report = {"cpython_commit": commit, "named_provider_controls": "matched CPython", "opt_level": args.opt_level, "whole_native_methods": sum(r["status"] == "pass" for r in native.values()),
              "tests": cases}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    for case, row in cases.items():
        print(row["status"], case)
        for diagnostic in row.get("diagnostics", ()):
            print(diagnostic, file=sys.stderr)
    return int(any(row["status"] == "fail" for row in cases.values()))


if __name__ == "__main__":
    sys.exit(main())
