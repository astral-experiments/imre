"""Run complete pinned unittest methods against native Imre at both optimization levels."""
import argparse
from collections import Counter
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_method, run_in_fixtures, testcase_fixture

PREFIX = "test.test_unittest."
ASSERTIONS = {
    "testAssertTrue": "assertTrue", "testAssertFalse": "assertFalse",
    "testAssertLess": "assertLess", "testAssertLessEqual": "assertLessEqual",
    "testAssertGreater": "assertGreater", "testAssertGreaterEqual": "assertGreaterEqual",
    "testNotEqual": "assertNotEqual", "test_baseAssertEqual": "_baseAssertEqual",
}


def check_assertion(binary, operation):
    def run(case, original):
        calls = []
        def bind(target):
            def call(*operands, msg=None):
                if type(target.longMessage) is not bool or msg is not None and type(msg) is not str:
                    raise TypeError("unexpected assertion fixture")
                unary = operation in {"assertTrue", "assertFalse"}
                if len(operands) != (1 if unary else 2) or not all(
                    type(value) is (bool if unary else int) for value in operands
                ):
                    raise TypeError("unexpected native assertion operands")
                invocation = [binary, operation, "long" if target.longMessage else "short",
                              "absent" if msg is None else "message", msg or "", *map(str, operands)]
                result = subprocess.run(invocation, capture_output=True, text=True, timeout=30)
                if result.returncode or result.stderr:
                    raise RuntimeError(f"native exit {result.returncode}: {result.stderr}")
                calls.append((target.longMessage, msg is not None))
                if result.stdout.startswith("failure\n") and result.stdout.endswith("\n"):
                    raise AssertionError(result.stdout[len("failure\n"):-1])
                if result.stdout != "success\n":
                    raise RuntimeError(f"invalid assertion response {result.stdout!r}")
            return call
        for target in (case.testableTrue, case.testableFalse):
            setattr(target, operation, bind(target))
        original()
        if Counter(calls) != Counter((long, msg) for long in (False, True) for msg in (False, True)):
            raise AssertionError(f"incomplete native calls: {calls}")
        return {"execution": "host-driven-native-operation", "body": "CPython",
                "assertions": "CPython checks the native failure message",
                "fixture_execution": "CPython", "operation": operation, "calls": len(calls)}
    return run


def run(args, level, root):
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    reference = json.loads((HERE / "reference-linux.json").read_text())
    lang = Toolchain(args.imre, args.runtime, args.clang, level)
    driver = root / "assertions"
    driver.mkdir()
    binary = lang.build((HERE / "assertion_driver.py").read_bytes(), driver)
    executions = {PREFIX + "test_assertions.TestLongMessage." + name: check_assertion(binary, op)
                  for name, op in ASSERTIONS.items()}
    path = HERE / "upstream/Lib/test/test_unittest/test_result.py"
    for method in ("test_stop", "test_startTestRun_stopTestRun"):
        case_id = PREFIX + "test_result.Test_TestResult." + method
        directory = root / method
        directory.mkdir()
        fixture = testcase_fixture("bool" if method == "test_stop" else None)
        executions[case_id] = native_method(lang, directory, path, "Test_TestResult", case_id, fixture)
    evidence, failures, skips = run_in_fixtures(load_suite(manifest["module"]), executions)
    if skips:
        failures.extend((name, f"unexpected skip: {reason}") for name, reason in skips)
    statuses = {name: "reference-skip" if isinstance(value, dict) and "skipped" in value
                else "reference-only" for name, value in reference["reference"].items()}
    for name, entries in evidence.items():
        entry, = entries
        statuses[name] = entry["execution"]
    for name, reason in failures:
        if name in statuses:
            statuses[name] = "failed"
        print(name, reason, file=sys.stderr)
    missing = set(executions) - set(evidence) - {name for name, _ in failures}
    if missing:
        raise RuntimeError(f"selected IDs did not execute: {sorted(missing)}")
    return {"cpython_commit": manifest["commit"], "opt_level": level, "denominator": len(statuses),
            "counts": dict(sorted(Counter(statuses.values()).items())), "evidence": evidence,
            "failures": failures}, bool(failures)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + option, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        parser.error("run with test-enabled CPython 3.14.5")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    reports = []
    failed = False
    for level in (0, 3):
        with tempfile.TemporaryDirectory() as work:
            report, bad = run(args, level, Path(work))
        print(f"-O{level}: {report['counts']}")
        failed |= bad
        reports.append(report)
    args.result.write_text(json.dumps(reports, indent=2) + "\n")
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
