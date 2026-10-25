"""Run four unmodified CPython JSON error tests with native Imre loads and host assertions."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from upstream import verify_upstream

METHODS = {
    "test_nonascii_digits_rejected": 3, "test_constant_invalid_case": 6,
    "test_extra_data": 1, "test_invalid_escape": 1,
}


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def vendored_decode(source):
    if source:
        sys.path.insert(0, str(source.resolve() / "Lib"))
    import test  # The pinned interpreter supplies test.support and the oracle machinery.
    vendored = HERE / "upstream/Lib/test"
    test.__path__.insert(0, str(vendored))
    from test.test_json import test_decode
    if Path(test_decode.__file__).resolve() != vendored / "test_json/test_decode.py":
        raise RuntimeError("the selected JSON tests must come from the verified vendor tree")
    return test_decode


def invoke(program, document, calls):
    completed = subprocess.run([program, document], capture_output=True, text=True,
                               encoding="utf-8", timeout=10)
    calls.append({"document": document, "stdout": completed.stdout, "stderr": completed.stderr,
                  "returncode": completed.returncode})
    if completed.returncode or completed.stderr or not completed.stdout.endswith("\n"):
        raise RuntimeError(f"native JSON driver failed: {calls[-1]}")
    tag, message = completed.stdout[:-1].split("\n", 1)
    return tag, message


def native_case(case, program, expected_calls):
    calls = []

    def loads(document):
        tag, message = invoke(program, document, calls)
        if tag == "JSONDecodeError":
            # Preserve the native exception category and str(error). Calling CPython's
            # initializer would synthesize location fields which Imre has not exposed.
            error = case.JSONDecodeError.__new__(case.JSONDecodeError)
            ValueError.__init__(error, message)
            raise error
        raise AssertionError(f"expected a native JSONDecodeError, got {tag}: {message}")

    # These pinned methods call only self.loads. An unsupported operation must fail.
    case.loads = loads
    case.json = case.dumps = None
    result = unittest.TestResult()
    case.run(result)
    passed = (result.testsRun == 1 and result.wasSuccessful() and not result.skipped
              and len(calls) == expected_calls)
    return {"status": "pass" if passed else "fail", "native_calls": calls,
            "diagnostics": [msg for _, msg in [*result.errors, *result.failures, *result.skipped]]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path,
                        help="CPython 3.14.5 source root if the interpreter lacks test.support")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use CPython 3.14.5 for the original unittest assertions")

    tests = {}
    report = {"cpython_commit": verify_upstream(), "assertions": "host unittest",
              "json_operations": "native Imre", "behavioral_methods": len(METHODS),
              "controls": [], "tests": tests}
    try:
        decode = vendored_decode(args.cpython_source)
        source = HERE / "json_driver.py"
        report["sha256"] = {name: sha256(path) for name, path in (
            ("driver", source), ("imre", args.imre), ("runtime", args.runtime), ("clang", args.clang))}
        with tempfile.TemporaryDirectory() as tmp:
            program = Path(tmp) / "json-driver"
            ir = Path(tmp) / "driver.ll"
            compiled = subprocess.run([args.imre.resolve(), "build", source, "-o", program,
                    "--no-cache", "--opt-level", "3", "--clang", args.clang.resolve(),
                    "--runtime", args.runtime.resolve(), "--emit-llvm", ir], cwd=tmp,
                    capture_output=True, text=True, timeout=120)
            if compiled.returncode:
                raise RuntimeError(f"Imre build failed: {compiled.stderr}")
            if "call ptr @imre_json_loads(" not in ir.read_text():
                raise RuntimeError("the compiled driver must call imre_json_loads")
            report["sha256"]["executable"] = sha256(program)
            for text, expected in [('[]', ("array-length", "0")),
                                   ('[{"a": 1}, 99]', ("array-length", "2"))]:
                if invoke(program, text, report["controls"]) != expected:
                    raise RuntimeError(f"unexpected native control result for {text}")
            for method, count in METHODS.items():
                for cls in (decode.TestPyDecode, decode.TestCDecode):
                    case = cls(method)
                    tests[case.id()] = native_case(case, program, count)
                    print(tests[case.id()]["status"], case.id())
    except (ImportError, OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        report["error"] = str(error)
        print(error, file=sys.stderr)
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int("error" in report or any(case["status"] != "pass" for case in tests.values()))


if __name__ == "__main__":
    sys.exit(main())
