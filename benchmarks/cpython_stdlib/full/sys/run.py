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
from suite_native import Toolchain, native_method, run_in_fixtures

MODULE = "test.test_sys"
SYS = MODULE + ".SysModuleTest."
EXCEPTION = MODULE + ".ActiveExceptionTests."

# The native TestCase currently accepts ints for assertEqual. These are the
# operand types of the original assertions; the upstream definitions stay intact.
STRING_EQUAL = """import sys

class NativeCase:
    def assertEqual(self, actual: str, expected: str) -> None:
        if actual != expected:
            raise AssertionError("strings differ")
"""

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


def select(toolchain, work, source, module):
    executions = {}
    for cls, method, fixture in [
        ("SysModuleTest", "test_43581", STRING_EQUAL),
        ("ActiveExceptionTests", "test_sys_exception_no_exception", EXCEPTION_EQUAL),
    ]:
        case_id = f"{MODULE}.{cls}.{method}"
        executions[case_id] = native_method(toolchain, directory(work, method),
                                             source, cls, case_id, fixture)

    # Independent object-identity regression; never counts as an upstream ID.
    exercise = HERE / "active_exception.py"
    ref = subprocess.run([sys.executable, exercise], capture_output=True, check=True, timeout=15)
    binary = toolchain.build(exercise.read_bytes(), directory(work, "exception-identity"))
    if output(binary) != (ref.stdout, ref.stderr):
        raise AssertionError("native exception identity differs from CPython")

    encoding = toolchain.build((HERE / "encoding_driver.py").read_bytes(), directory(work, "encoding"))

    def encoding_case(case, original):
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
            @property
            def platform(self):
                return invoke("platform")

            def getdefaultencoding(self, *args):
                return invoke("default-argument" if args else "default", *args)

            def getfilesystemencoding(self):
                return invoke("filesystem")

        with patch.object(module, "sys", NativeSys()):
            original()
        if not calls:
            raise AssertionError(f"{case.id()} made no native calls")
        return {"execution": "host-driven-native", "body": "CPython", "assertions": "CPython",
                "fixture_execution": "CPython module/class/instance hooks", "calls": calls}

    for method in ("test_getdefaultencoding", "test_getfilesystemencoding"):
        executions[SYS + method] = encoding_case

    def nonascii(case, original):
        sources = []

        def popen(argv, **kwargs):
            if len(argv) != 3 or argv[:2] != [sys.executable, "-c"]:
                raise ValueError(f"unexpected child: {argv!r}")
            data = argv[2].encode()
            child = toolchain.build(data, directory(work, "nonascii-child"))
            sources.append(hashlib.sha256(data).hexdigest())
            return subprocess.Popen([child], **kwargs)

        with patch.object(module, "subprocess", SimpleNamespace(Popen=popen, PIPE=subprocess.PIPE)):
            original()
        if len(sources) != 1:
            raise AssertionError(sources)
        return {"execution": "host-driven-native", "body": "CPython; child source: Imre",
                "assertions": "CPython", "fixture_execution": "CPython module/class/instance hooks",
                "child_sha256": sources[0]}

    executions[SYS + "test_ioencoding_nonascii"] = nonascii
    return executions


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
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
        executions = select(toolchain, Path(tmp), source, test_sys)
        evidence, failures, skips = run_in_fixtures(suite, executions)
    if failures:
        raise AssertionError(failures)
    results = {}
    for case_id, occurrences in evidence.items():
        item, = occurrences
        results[case_id] = {"status": "pass", **item}
    for case_id, reason in skips:
        results[case_id] = {"status": "upstream-skip", "reason": reason}
    if set(results) != executions.keys():
        raise ValueError("missing execution outcome")
    args.result.write_text(json.dumps({"cpython_commit": manifest["commit"],
                                       "opt_level": args.opt_level, "denominator": len(ids),
                                       "tests": dict(sorted(results.items()))}, indent=2) + "\n")


if __name__ == "__main__":
    main()
