"""Run verified test_sys methods in their upstream fixtures at one optimization level."""

import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path[:0] = [str(HERE), str(STDLIB)]
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import NativeCompileError, Toolchain, native_method, run_evidence, run_in_fixtures, testcase_fixture
from suite_evidence import roles
from occurrences import blocked_methods, reference_skips
import argv_case
import encoding_methods
import jit_case
import locale_case
import recursion_case
import thread_case
from filesystem_case import filesystem_method
from metadata import metadata_observer

MODULE = "test.test_sys"
SYS = MODULE + ".SysModuleTest."
EXCEPTION = MODULE + ".ActiveExceptionTests."

# The shared TestCase cannot yet compare an optional exception with None.
EXCEPTION_EQUAL = """import sys

class NativeCase:
    def assertEqual(self, actual: BaseException | None, expected: None) -> None:
        if actual is not expected:
            raise AssertionError("exception differs from None")
"""


def directory(parent, name):
    path = parent / name
    path.mkdir()
    return path


def output(binary):
    result = subprocess.run([binary], capture_output=True, check=True, timeout=15)
    return result.stdout, result.stderr


def method_hashes(source):
    lines = source.read_bytes().splitlines(keepends=True)
    return {(cls.name, method.name): hashlib.sha256(b"".join(
        lines[min([method.lineno, *(d.lineno for d in method.decorator_list)]) - 1:method.end_lineno]
    )).hexdigest() for cls in ast.parse(b"".join(lines)).body if isinstance(cls, ast.ClassDef)
       for method in cls.body if isinstance(method, ast.FunctionDef)}


def select(toolchain, work, source, module):
    executions = {}
    pinned_methods = method_hashes(source)
    # A host process cannot supply frames to a native child. Compare the stacks
    # of two complete programs with the same calls, separately from upstream IDs.
    frames = directory(work, "frame-names")
    owner = HERE / "frame_names_owner.py"
    (frames / owner.name).write_bytes(owner.read_bytes())
    binary = toolchain.build((HERE / "frame_names.py").read_bytes(), frames)
    stock = subprocess.run([sys.executable, "-E", "-B", frames / "case.py"],
                           capture_output=True, check=True, timeout=15)
    actual = output(binary)
    if actual != (stock.stdout, stock.stderr):
        raise AssertionError(("native module frames", (stock.stdout, stock.stderr), actual))
    frame_control = {"kind": "separate native program", "opt_level": toolchain.level,
                     "source": "frame_names.py and frame_names_owner.py",
                     "stdout_sha256": hashlib.sha256(actual[0]).hexdigest(),
                     "stdout_lines": len(actual[0].splitlines()), "matches_cpython": True,
                     "original_method_operations": 0}
    for cls, method, fixture, operation in [
        ("SysModuleTest", "test_43581", "import sys\n" + testcase_fixture(), "sys.__stdout__.encoding and sys.__stderr__.encoding"),
        ("ActiveExceptionTests", "test_sys_exception_no_exception", EXCEPTION_EQUAL, "sys.exception() outside a handler"),
    ]:
        case_id = f"{MODULE}.{cls}.{method}"
        executions[case_id] = native_method(toolchain, directory(work, method),
                                             source, cls, case_id, fixture, operation=("imre", operation))

    thread = toolchain.build((HERE / "thread_info.py").read_bytes(), directory(work, "thread-info"))
    thread_case.check_record(thread, toolchain.clang)
    executions[SYS + "test_thread_info"] = thread_case.observer(
        module, thread, pinned_methods["SysModuleTest", "test_thread_info"], toolchain.level)
    impl = toolchain.build((HERE / "metadata_driver.py").read_bytes(), directory(work, "implementation"))
    executions[SYS + "test_implementation"] = metadata_observer(module, impl, toolchain.level)
    identity = subprocess.run([impl, "identity"], check=True, capture_output=True, text=True, timeout=15)
    lines = identity.stdout.splitlines()
    if identity.stderr or len(lines) != 4 or lines[:3] != ["True", "True", "True"]:
        raise AssertionError(identity)
    if not lines[3].startswith("namespace(name='imre', cache_tag=None, version=sys.version_info("):
        raise AssertionError(lines[3])
    # A cache tag assigned through either alias must also reach importlib and namespace repr.
    command = ["cache_alias", "shared-cache-unit"]
    native = subprocess.run([impl, *command], capture_output=True, check=True, timeout=15)
    control = subprocess.run([sys.executable, "-I", HERE / "metadata_driver.py", *command],
                             capture_output=True, check=True, timeout=15)
    if (native.stdout, native.stderr) != (control.stdout, control.stderr):
        raise AssertionError((native, control))

    # Both implementations check internal consistency using their own language version.
    metadata = HERE / "version_info.py"
    version_work = directory(work, "language-version")
    owner = HERE / "version_info_owner.py"
    (version_work / owner.name).write_bytes(owner.read_bytes())
    program = toolchain.build(metadata.read_bytes(), version_work)
    reference = subprocess.run([sys.executable, "-E", "-B", version_work / "case.py"],
                               capture_output=True, check=True, timeout=15)
    if (reference.stdout, reference.stderr) != (b"", b"") or output(program) != (b"", b""):
        raise AssertionError("unexpected output from language version checks")

    # Process regressions use the real hooks and retained objects; none adds a test ID.
    for filename in ("active_exception.py", "display_values.py", "codec_lookup.py", "codec_callbacks.py", "codec_error_args.py"):
        exercise = HERE / filename
        ref = subprocess.run([sys.executable, exercise], capture_output=True, check=True, timeout=15)
        binary = toolchain.build(exercise.read_bytes(), directory(work, exercise.stem))
        if output(binary) != (ref.stdout, ref.stderr):
            raise AssertionError(f"{filename}: native output differs from CPython")

    exercise = HERE / "intern_values.py"
    ref = subprocess.run([sys.executable, "-B", exercise], capture_output=True, check=True, timeout=15)
    binary = toolchain.build(exercise.read_bytes(), directory(work, "intern-values"))
    if output(binary) != (ref.stdout, ref.stderr):
        raise AssertionError("native string interning differs from CPython")

    invalid = HERE / "intern-binder/intern-nested-invalid.py"
    folder = directory(work, "intern-nested-invalid")
    try:
        toolchain.build(invalid.read_bytes(), folder)
    except NativeCompileError as error:
        expected = f'{folder}/case.py:6:23: Imre: parameter "value" is positional-only'
        if str(error).splitlines()[-1] != expected:
            raise AssertionError(("nested sys.intern diagnostic", str(error))) from error
    else:
        raise AssertionError("sys.intern accepted a bad call inside its argument")

    encoding = toolchain.build((HERE / "encoding_driver.py").read_bytes(), directory(work, "encoding"))

    def default_encoding_case(case, original):
        calls = []

        def invoke(operation, *values):
            run = subprocess.run([encoding, operation, *map(str, values)],
                                 capture_output=True, timeout=15)
            calls.append(operation)
            if run.returncode == 1 and not run.stdout and run.stderr.startswith(b"TypeError: "):
                raise TypeError(run.stderr.removeprefix(b"TypeError: ").decode().rstrip("\n"))
            run.check_returncode()
            if run.stderr:
                raise AssertionError(run.stderr)
            return ast.literal_eval(run.stdout.decode())

        class NativeSys:
            def getdefaultencoding(self, *args):
                return invoke("default-argument" if args else "default", *args)

        with patch.object(module, "sys", NativeSys()):
            original()
        if calls != ["default-argument", "default"]:
            raise AssertionError((case.id(), calls))
        return {"execution": "host-driven-native", "body": "CPython", "assertions": "CPython",
                "fixture_execution": "CPython module/class/instance hooks", "calls": calls,
                "observation": {"roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                                "evidence": {"operation": "sys.getdefaultencoding",
                                             "assertions": "upstream TypeError and result type assertions",
                                             "method_sha256": pinned_methods[type(case).__name__, case._testMethodName],
                                             "fixture": "original SysModuleTest lifecycle and reap_children teardown",
                                             "native_calls": len(calls), "calls": calls, "opt_level": toolchain.level}}}

    default_id = SYS + "test_getdefaultencoding"
    executions[default_id] = encoding_methods.select(toolchain, work, source, default_id, default_encoding_case)
    fs_id = SYS + "test_getfilesystemencoding"
    executions[fs_id] = filesystem_method(toolchain, directory(work, "filesystem"), source, fs_id)

    executions[argv_case.CASE_ID] = argv_case.observer(
        module, toolchain, work / "argv-controls", pinned_methods["SysModuleTest", "test_orig_argv"])
    executions[locale_case.CASE_ID] = locale_case.observer(
        module, toolchain, work / "nonascii-child", pinned_methods["SysModuleTest", "test_ioencoding_nonascii"])
    executions.update(recursion_case.cases(module, toolchain, work, source))
    jit = jit_case.observer(module, toolchain, work, pinned_methods)
    for method in ("test_jit_is_available", "test_jit_is_enabled"):
        executions[f"{MODULE}.TestSysJIT.{method}"] = jit
    return executions, frame_control


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    parser.add_argument("--cpython-source", type=Path)
    parser.add_argument("--evidence", type=Path, help="write the versioned report for this optimization level")
    parser.add_argument("--result", type=Path, help="write legacy full-suite outcomes")
    parser.add_argument("--locale-audit", type=Path, help="compare this host configuration on the one unchanged locale case")
    args = parser.parse_args()
    if bool(args.locale_audit) == bool(args.result) or (args.locale_audit and args.evidence):
        parser.error("choose --result [--evidence] or --locale-audit")
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("run with the test-enabled CPython 3.14.5 release")
    manifest = json.loads((STDLIB / "UPSTREAM.json").read_text())
    filenames = ["LICENSE", "Lib/test/test_sys.py"]
    files = {name: manifest["files"][name] for name in filenames}
    verify_files(STDLIB / "upstream", files)
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    verify_interpreter_tests({"files": files})
    import test
    vendored = STDLIB / "upstream/Lib/test"
    test.__path__.insert(0, str(vendored))
    from test import test_sys
    source = vendored / "test_sys.py"
    if Path(test_sys.__file__).resolve() != source:
        raise ValueError("test_sys was not loaded from its verified vendored file")
    reference = json.loads((HERE / "reference-linux.json").read_text())
    suite = load_suite(MODULE)
    ids = [case.id() for case in cases(suite)]
    if (sorted(ids) != sorted(reference["reference"]) or len(ids) != reference["denominator"]
            or manifest["commit"] != reference["cpython_commit"]):
        raise ValueError("discovered IDs or upstream revision differ from the reference")
    toolchain = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    with tempfile.TemporaryDirectory(prefix="imre-test-sys-") as tmp:
        if args.locale_audit:
            hash_ = method_hashes(source)["SysModuleTest", "test_ioencoding_nonascii"]
            locale_case.audit(args.locale_audit, test_sys, toolchain, Path(tmp) / "child", hash_, manifest["commit"])
            return
        if args.evidence:
            config = locale_case.settings()
            if (config["LC_CTYPE"], config["utf8_mode"], config["filesystem_encoding"],
                    config["preferred_encoding"]) != ("C.UTF-8", 0, "utf-8", "UTF-8"):
                parser.error(f"canonical Linux report requires the recorded C.UTF-8 reference settings: {config}")
        executions, frame_control = select(toolchain, Path(tmp), source, test_sys)
        if args.evidence:
            preflight = blocked_methods(toolchain, Path(tmp), source, reference, executions, test_sys)
            preflight[SYS + "test_getframemodulename", 1]["evidence"]["module_name_control"] = frame_control
            jit_case.audit_active(load_suite(MODULE), executions, preflight)
            evidence = {}

            def capture(callback):
                def execute(case, original):
                    record = callback(case, original)
                    evidence.setdefault(case.id(), []).append(record)
                    return record
                return execute

            report = run_evidence(suite, {name: capture(fn) for name, fn in executions.items()},
                                  reference, args.opt_level, preflight=preflight,
                                  exceptions=reference_skips(reference, args.opt_level))
            args.evidence.write_text(json.dumps(report, indent=2) + "\n")
            selected = [r for r in report["records"] if r["id"] in executions]
            failures = [r for r in selected if r["outcome"] not in {"pass", "skip"}]
            skips = [(r["id"], r["reason"]) for r in selected if r["outcome"] == "skip"]
        else:
            evidence, failures, skips = run_in_fixtures(suite, executions)
    if failures:
        raise AssertionError(failures)
    results = {}
    for case_id, occurrences in evidence.items():
        item, = occurrences
        results[case_id] = {"status": "pass", **{k: v for k, v in item.items() if k != "observation"}}
    for case_id, reason in skips:
        results[case_id] = {"status": "upstream-skip", "reason": reason}
    if set(results) != executions.keys():
        raise ValueError("missing execution outcome")
    args.result.write_text(json.dumps({"cpython_commit": manifest["commit"],
                                       "opt_level": args.opt_level, "denominator": len(ids),
                                       "tests": dict(sorted(results.items()))}, indent=2) + "\n")


if __name__ == "__main__":
    main()
