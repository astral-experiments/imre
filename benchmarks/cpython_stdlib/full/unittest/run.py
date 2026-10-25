"""Run complete pinned unittest methods against native Imre at both optimization levels."""
import argparse
import ast
import hashlib
from collections import Counter
from contextlib import contextmanager
from dataclasses import replace
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from source_modules import verify as verify_source_modules
from suite_evidence import roles, write_reports
from suite_native import (NativeCompileError, NativeDefaultMethod, Toolchain, compile_blocker,
                          method_bytes, native_method, run_evidence, testcase_fixture)
import patch_cases

PREFIX = "test.test_unittest."
def loader_methods():
    path = HERE / "upstream/Lib/test/test_unittest/test_loader.py"
    owner, = (c for c in ast.parse(path.read_bytes()).body
              if isinstance(c, ast.ClassDef) and c.name == "Test_TestLoader")
    methods = {m.name: "unittest.TestLoader.getTestCaseNames"
               for m in owner.body if isinstance(m, ast.FunctionDef)
               and any(isinstance(call, ast.Call) and isinstance(call.func, ast.Attribute)
                       and call.func.attr == "getTestCaseNames" for call in ast.walk(m))}
    methods["test___init__"] = "unittest.TestLoader construction and its errors list"
    return methods


DEFAULT_LOADER_METHODS = loader_methods()
# These methods still run on CPython. Keep the native attempt in each report;
# a different compiler failure must fail the gate.
LOADER_BLOCKERS = {
    "test_getTestCaseNames__not_a_TestCase": "5:23: Imre: local class base requires supported native class metadata",
    "test_getTestCaseNames__testNamePatterns__attribute_access_regression":
        "6:13: Imre: local class body requires plain methods without defaults or inert class attributes",
    "test_partial_functions": "6:9: Imre: local functions require annotated parameters and return types",
    "test_sortTestMethodsUsing__None": "10:9: Imre: unknown field; dynamic attributes and first-class methods are unsupported",
    "test_sortTestMethodsUsing__getTestCaseNames": "5:9: Imre: local functions require annotated parameters and return types",
}
ASSERTIONS = {
    "testAssertTrue": "assertTrue", "testAssertFalse": "assertFalse",
    "testAssertLess": "assertLess", "testAssertLessEqual": "assertLessEqual",
    "testAssertGreater": "assertGreater", "testAssertGreaterEqual": "assertGreaterEqual",
    "testNotEqual": "assertNotEqual", "test_baseAssertEqual": "_baseAssertEqual",
    "testAssertRaises": "assertRaises", "testAssertDictEqual": "assertDictEqual",
    "testAssertIsNone": "assertIsNone", "testAssertIsNotNone": "assertIsNotNone",
}
# These CPython passes still fail at native compilation. Keep their first
# precise source diagnostic in the report; a different failure stops the gate.
EQUALITY_BLOCKERS = {
    "testAssertMultiLineEqualTruncates": (1016, "6:34: Imre: too many positional arguments"),
    "testAssertEqual_diffThreshold": (1036, "14:9: Imre: unknown class method"),
    "testAssertEqual_shorten": (1067, "9:9: Imre: unknown class method"),
}
# Each entry has a reviewed native product operation and the original default fixture.
DEFAULT_MOCK_METHODS = {
    "test_assert_not_called": "unittest.mock.Mock.assert_not_called",
    "test_assert_called": "unittest.mock.Mock.assert_called",
    "test_assert_called_once": "unittest.mock.Mock.assert_called_once",
    "test_assert_called_once_message_not_called":
        "unittest.TestCase.assertNotIn and the message from unittest.mock.Mock.assert_called_once",
    "test_ordered_call_signature": "unittest.mock.Mock.call_args and its representation",
    "test_assert_any_call": "unittest.TestCase.assertRaises and unittest.mock.Mock.assert_any_call",
    "test_assert_called_with": "unittest.TestCase.assertRaises and unittest.mock.Mock.assert_called_with",
    "test_assert_called_once_with": "unittest.TestCase.assertRaises and unittest.mock.Mock.assert_called_once_with",
}
MOCK_BLOCKERS = {
    "test_reset_mock": (337, '9:35: Imre: unexpected keyword argument "parent"'),
    "test_reset_mock_recursion": (377, "8:9: Imre: properties are read-only"),
    "test_reset_sideeffect": (1890, '7:18: Imre: unexpected keyword argument "return_value"'),
}
CALLABLE_BLOCKERS = {
    "testAssertRaisesCallable": (1457, "6:29: Imre: local class base requires supported native class metadata"),
    "test_assertRaises": (61, "6:9: Imre: local functions require annotated parameters and return types"),
}


def check_assertion(binary, operation, level):
    def run(case, original):
        calls = []
        def bind(target):
            def invoke(operands, msg):
                if type(target.longMessage) is not bool or msg is not None and type(msg) is not str:
                    raise TypeError("unexpected assertion fixture")
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
            @contextmanager
            def scope(expected, *, msg=None):
                if expected is not TypeError:
                    raise TypeError("native context driver expects TypeError")
                # The unchanged host callback must return normally. Only this
                # outcome is represented by the native driver's empty protected block.
                yield
                invoke((), msg)
            if operation == "assertRaises":
                return scope
            def call(*operands, msg=None):
                if operation == "assertDictEqual":
                    if (len(operands) != 2 or any(type(d) is not dict for d in operands)
                            or any(type(s) is not str for d in operands for pair in d.items() for s in pair)):
                        raise TypeError("expected two string dictionaries")
                    invoke([*(len(d) for d in operands),
                            *(s for d in operands for pair in d.items() for s in pair)], msg)
                    return
                if operation in {"assertIsNone", "assertIsNotNone"}:
                    if len(operands) != 1 or (operands[0] is not None and type(operands[0]) is not str):
                        raise TypeError("unexpected native None assertion operand")
                    invoke(("none",) if operands[0] is None else ("str", operands[0]), msg)
                    return
                unary = operation in {"assertTrue", "assertFalse"}
                if len(operands) != (1 if unary else 2) or not all(
                    type(value) is (bool if unary else int) for value in operands
                ):
                    raise TypeError("unexpected native assertion operands")
                invoke(operands, msg)
            return call
        for target in (case.testableTrue, case.testableFalse):
            setattr(target, operation, bind(target))
        original()
        if Counter(calls) != Counter((long, msg) for long in (False, True) for msg in (False, True)):
            raise AssertionError(f"incomplete native calls: {calls}")
        return {"roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                "evidence": {"operation": f"unittest.TestCase.{operation}", "native_calls": len(calls), "opt_level": level,
                             "assertions": "upstream unittest helper checks each native failure message",
                             "fixture": "upstream TestLongMessage setup and lifecycle on CPython"}}
    return run


def run(args, level, root, fixture_root):
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    reference = json.loads((HERE / "reference-linux.json").read_text())
    if (reference["module"], reference["cpython_commit"]) != (manifest["module"], manifest["commit"]):
        raise ValueError("reference does not match the pinned unittest manifest")
    lang = Toolchain(args.imre, args.runtime, args.clang, level, args.reachable_only)
    verify_source_modules(lang, root)
    patches = patch_cases.methods()
    patch_compiler = replace(lang, module_root=fixture_root)
    families = dict.fromkeys(["protocol", "import", "csv", *args.exception_family])
    for name in ["truth_protocol", "equality_protocol", "membership", "mapping_protocol", "dictionary_patch", "boolean_equality", "mock_protocol", "mock_reset", "mock_globals", "callable_protocol", "bound_assertions",
                 "callable_binding", "context_regex", "loader_names", "none_assertions", *("context_" + family for family in families)]:
        control = root / name
        if (HERE / name).is_dir():
            shutil.copytree(HERE / name, control)
            program = control / "case.py"
        else:
            control.mkdir()
            program = HERE / (name + ".py")
        if name == "loader_names":
            shutil.copytree(HERE / "upstream/Lib/test/test_unittest/namespace_test_pkg/bar", control / "bar")
        native = lang.build(program.read_bytes(), control)
        oracle = subprocess.run([sys.executable, "-E", "-B", control / "case.py"], cwd=control,
                                capture_output=True, timeout=30, check=True)
        observed = subprocess.run([native], capture_output=True, timeout=30, check=True)
        if (observed.stdout, observed.stderr) != (oracle.stdout, oracle.stderr):
            raise AssertionError(f"{program.name} differs from CPython: {observed!r} != {oracle!r}")
    driver = root / "assertions"
    driver.mkdir()
    executions, native_defaults, blocked, measured_blockers = {}, {}, {}, set()
    expected_loader_blocks = {}
    host = {PREFIX + "test_assertions.TestLongMessage." + name: op for name, op in ASSERTIONS.items()}
    try:
        binary = lang.build((HERE / "assertion_driver.py").read_bytes(), driver)
    except NativeCompileError as exc:
        blocked.update({(name, 1): compile_blocker(exc) for name in host})
    else:
        executions.update({name: check_assertion(binary, op, level) for name, op in host.items()})
    for module, owner, method, operation in (
        *(("test_loader", "Test_TestLoader", method, operation) for method, operation in DEFAULT_LOADER_METHODS.items()),
        ("test_result", "Test_TestResult", "test_stop", "unittest.TestResult.stop"),
        ("test_result", "Test_TestResult", "test_startTestRun_stopTestRun", "unittest.TestResult.startTestRun and stopTestRun"),
        ("test_case", "Test_TestCase", "testAssertRaisesNoExceptionType", "unittest.TestCase.assertRaises"),
        ("test_case", "Test_TestCase", "testAssertRaisesCallable", "unittest.TestCase.assertRaises"),
        ("test_case", "Test_TestCase", "testAssertIsNone", "unittest.TestCase.assertIsNone and assertIsNotNone"),
        ("test_assertions", "Test_Assertions", "test_assertRaises", "unittest.TestCase.assertRaises"),
        *(("test_case", "Test_TestCase", method, "unittest.TestCase string equality and failure messages")
          for method in ("testAssertEqualSingleLine", "testAssertEqualwithEmptyString",
                         "testAssertEqualMultipleLinesMissingNewlineTerminator",
                         "testAssertEqualMultipleLinesMismatchedNewlinesTerminators",
                         "testAssertMultiLineEqual", *EQUALITY_BLOCKERS)),
        *(("testmock.testpatch", "PatchTest", method, "unittest.mock.patch.dict and restoration") for method in patches),
        *(("testmock.testmock", "MockTest", method, operation) for method, operation in DEFAULT_MOCK_METHODS.items()),
        ("testmock.testmock", "MockTest", "test_assert_called_exception_message",
         "unittest.TestCase.assertRaisesRegex and unittest.mock.Mock.assert_called"),
        *(("testmock.testmock", "MockTest", method, "unittest.mock recording, reset, and call assertions")
          for method in MOCK_BLOCKERS),
    ):
        path = HERE / "upstream/Lib/test/test_unittest" / (module.replace(".", "/") + ".py")
        case_id = PREFIX + f"{module}.{owner}.{method}"
        directory = fixture_root / f"{method}_O{level}" if module == "testmock.testpatch" else root / method
        directory.mkdir()
        default_fixture = ((module == "testmock.testmock" and method in DEFAULT_MOCK_METHODS)
                           or (module == "test_loader" and method in DEFAULT_LOADER_METHODS))
        if default_fixture or module == "testmock.testpatch":
            blob = manifest["files"][path.relative_to(HERE / "upstream").as_posix()]
            compiler = lang
            if module == "testmock.testpatch":
                compiler = patch_compiler
                (directory / "__init__.py").touch()
                (directory / "test").symlink_to(fixture_root / "test", target_is_directory=True)
            native_defaults[case_id, 1] = NativeDefaultMethod(compiler, directory, path, blob, ("imre", operation))
            if module == "test_loader" and method in LOADER_BLOCKERS:
                diagnostic = f"Compiling <case>/case.py (-O{level})\n<case>/case.py:{LOADER_BLOCKERS[method]}"
                expected_loader_blocks[case_id, 1] = diagnostic, {
                    "stage": "compile", "source_blob": blob,
                    "method_sha256": hashlib.sha256(method_bytes(path, owner, method)).hexdigest(),
                }
            continue
        fixture = testcase_fixture()
        if module == "testmock.testmock":
            fixture = "from unittest.mock import Mock\n" + fixture
        try:
            executions[case_id] = native_method(lang, directory, path, owner, case_id, fixture,
                                                operation=("imre", operation))
        except NativeCompileError as exc:
            observation = compile_blocker(exc)
            observation["reason"] = str(exc).strip().replace(str(directory) + "/", "<case>/")
            expected = (MOCK_BLOCKERS.get(method) if module == "testmock.testmock"
                        else EQUALITY_BLOCKERS.get(method) or CALLABLE_BLOCKERS.get(method))
            if module == "testmock.testmock" and method == "test_assert_called_exception_message":
                # The original pattern formats positional {0} before entering assertRaisesRegex.
                expected = (533, "8:53: Imre: str.format requires named arguments")
            if expected:
                line, diagnostic = expected
                if observation["reason"].endswith("<case>/case.py:" + diagnostic):
                    measured_blockers.add((case_id, 1))
                observation["evidence"].update(
                    original_source=f"{path.relative_to(HERE / 'upstream')}:{line}",
                    method_sha256=hashlib.sha256(method_bytes(path, owner, method)).hexdigest(),
                )
            blocked[case_id, 1] = observation
    report = run_evidence(load_suite(manifest["module"]), executions, reference, level,
                          preflight=blocked, native_defaults=native_defaults, exceptions=reference_skips(level))
    for row in report["records"]:
        expected = expected_loader_blocks.get((row["id"], row["occurrence"]))
        if (expected and row["classification"] == "blocked" and row["reference"] == "pass"
                and (row["reason"].strip(), row["evidence"]) == expected):
            measured_blockers.add((row["id"], row["occurrence"]))
    previous = patch_cases.known_blockers(args.reachable_only, level)
    for row in report['records']:
        if row['id'].startswith(PREFIX + 'testmock.testpatch.PatchTest.'):
            method = row['id'].rsplit('.', 1)[-1]
            if method in patches:
                if patch_cases.recorded_blocker(row, previous.get((row['id'], row['occurrence'])), patches[method], fixture_root):
                    measured_blockers.add((row['id'], row['occurrence']))
                if row['outcome'] == 'pass' and method not in patch_cases.REVIEWED:
                    raise ValueError(f"{row['id']}: review the original operation and assertions before publishing a new pass")
    failures = [r for r in report["records"]
                if (r["outcome"] not in {"pass", "not-run"}
                    and not (r["outcome"] == "blocked" and (r["id"], r["occurrence"]) in measured_blockers))
                or r["classification"] in {"reference-failure", "reference-disagreement"}]
    for row in failures:
        print(row["id"], row.get("reason", row["reference"]), file=sys.stderr)
    # Retain original-method observations even when a separate control fails.
    from loader_class_boundaries import verify as verify_local_class_boundaries
    try:
        verify_local_class_boundaries(lang, root)
    except (NativeCompileError, AssertionError, subprocess.SubprocessError) as exc:
        print(f"local class discovery control failed at O{level}: {exc}", file=sys.stderr)
        return report, True
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
    parser.add_argument("--reachable-only", action="store_true", help="build all native entries in reachable-only mode")
    parser.add_argument("--exception-family", action="append", choices=("csv", "import"), default=[],
                        help="explicitly require a family control (csv and import are checked by default)")
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
    with tempfile.TemporaryDirectory() as fixture:
        fixture_root = Path(fixture)
        patch_cases.stage(fixture_root, args.cpython_source)
        for level in (0, 3):
            with tempfile.TemporaryDirectory() as work:
                report, bad = run(args, level, Path(work), fixture_root)
            print(f"-O{level}: {report['counts']}")
            failed |= bad
            reports.append(report)
    write_reports(args.result, reports, json.loads((HERE / "reference-linux.json").read_text()))
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
