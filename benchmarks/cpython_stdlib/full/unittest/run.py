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
from suite_evidence import roles, write_reports
from suite_native import NativeCompileError, Toolchain, compile_blocker, native_method, run_evidence, testcase_fixture

PREFIX = "test.test_unittest."
ASSERTIONS = {
    "testAssertTrue": "assertTrue", "testAssertFalse": "assertFalse",
    "testAssertLess": "assertLess", "testAssertLessEqual": "assertLessEqual",
    "testAssertGreater": "assertGreater", "testAssertGreaterEqual": "assertGreaterEqual",
    "testNotEqual": "assertNotEqual", "test_baseAssertEqual": "_baseAssertEqual",
}


def check_assertion(binary, operation, level):
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
        return {"roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                "evidence": {"operation": f"unittest.TestCase.{operation}", "native_calls": len(calls), "opt_level": level,
                             "assertions": "upstream assertMessages checks each native failure message",
                             "fixture": "upstream TestLongMessage setup and lifecycle on CPython"}}
    return run


def run(args, level, root):
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    reference = json.loads((HERE / "reference-linux.json").read_text())
    if (reference["module"], reference["cpython_commit"]) != (manifest["module"], manifest["commit"]):
        raise ValueError("reference does not match the pinned unittest manifest")
    lang = Toolchain(args.imre, args.runtime, args.clang, level)
    driver = root / "assertions"
    driver.mkdir()
    executions, blocked = {}, {}
    host = {PREFIX + "test_assertions.TestLongMessage." + name: op for name, op in ASSERTIONS.items()}
    try:
        binary = lang.build((HERE / "assertion_driver.py").read_bytes(), driver)
    except NativeCompileError as exc:
        blocked.update({(name, 1): compile_blocker(exc) for name in host})
    else:
        executions.update({name: check_assertion(binary, op, level) for name, op in host.items()})
    path = HERE / "upstream/Lib/test/test_unittest/test_result.py"
    for method, operation in (("test_stop", "unittest.TestResult.stop"),
                              ("test_startTestRun_stopTestRun", "unittest.TestResult.startTestRun and stopTestRun")):
        case_id = PREFIX + "test_result.Test_TestResult." + method
        directory = root / method
        directory.mkdir()
        fixture = testcase_fixture("bool" if method == "test_stop" else None)
        try:
            executions[case_id] = native_method(lang, directory, path, "Test_TestResult", case_id, fixture,
                                                operation=("imre", operation))
        except NativeCompileError as exc:
            blocked[case_id, 1] = compile_blocker(exc)
    report = run_evidence(load_suite(manifest["module"]), executions, reference, level,
                          preflight=blocked, exceptions=reference_skips(level))
    failures = [r for r in report["records"] if r["outcome"] not in {"pass", "not-run"}
                or r["classification"] in {"reference-failure", "reference-disagreement"}]
    for row in failures:
        print(row["id"], row.get("reason", row["reference"]), file=sys.stderr)
    return report, bool(failures)


def reference_skips(level):
    fixture = PREFIX + "test_break.TestBreakSignalIgnored"
    return {(fixture + "." + method, 1, level): {
        "kind": "upstream-skip", "fixture_variant": fixture, "platform": "linux",
        "configuration": "TestBreakSignalIgnored.int_handler = signal.SIG_IGN",
        "condition": "signal.getsignal(signal.SIGINT) == signal.SIG_IGN",
        "reason": "test requires SIGINT to not be ignored",
        "source": f"Lib/test/test_unittest/test_break.py:{line}",
    } for method, line in (("testSecondInterrupt", 102), ("testHandlerReplacedButCalled", 152))}


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
    write_reports(args.result, reports, json.loads((HERE / "reference-linux.json").read_text()))
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
