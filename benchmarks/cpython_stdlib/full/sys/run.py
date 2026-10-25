"""Run complete, verified test_sys routines with the native Imre compiler."""

import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from upstream import verify_upstream


def build(args, source, binary):
    subprocess.run([args.imre, "build", source, "--runtime", args.runtime, "--clang",
                    args.clang, "--opt-level", str(args.opt_level), "--no-cache",
                    "--quiet", "-o", binary], check=True, timeout=120)


def native_method(args, work, source, cls_name, method, assertions):
    data = source.read_bytes()
    cls = next(n for n in ast.parse(data).body if isinstance(n, ast.ClassDef)
               and n.name == cls_name)
    node = next(n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == method)
    if node.decorator_list:
        raise ValueError("unhandled upstream decorator")
    body = b"".join(data.splitlines(keepends=True)[node.lineno - 1:node.end_lineno])
    script = work / f"{method}.py"
    script.write_bytes(b"import sys\n\nclass Case:\n" + assertions.encode() + body
                       + f"\nCase().{method}()\n".encode())
    binary = work / method
    build(args, script, binary)
    result = subprocess.run([binary], capture_output=True, check=True, timeout=15)
    if (result.stdout, result.stderr) != (b"assertEqual\n", b""):
        raise AssertionError(result)
    return {"execution": "native-body", "status": "pass", "assertions": 1,
            "method_sha256": hashlib.sha256(body).hexdigest()}


STRING_EQUAL = """    def assertEqual(self, actual: str, expected: str) -> None:
        if actual != expected:
            raise ValueError("assertEqual")
        print("assertEqual")

"""

EXCEPTION_EQUAL = """    def assertEqual(self, actual: BaseException | None, expected: None) -> None:
        if actual is not expected:
            raise AssertionError("assertEqual")
        print("assertEqual")

"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("run the original unittest assertions with CPython 3.14.5")
    for key in ("imre", "runtime", "clang"):
        setattr(args, key, getattr(args, key).resolve())
    commit = verify_upstream()
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    vendored = STDLIB / "upstream/Lib/test"
    test.__path__.insert(0, str(vendored))
    from test import test_sys
    source = vendored / "test_sys.py"
    if Path(test_sys.__file__).resolve() != source:
        raise ValueError("test_sys was not loaded from the verified vendored source")
    reference = json.loads((HERE / "reference-linux.json").read_text())
    cases = {}
    with tempfile.TemporaryDirectory(prefix="imre-test-sys-") as tmp:
        work = Path(tmp)
        prefix = "test.test_sys.SysModuleTest."
        cases[prefix + "test_43581"] = native_method(
            args, work, source, "SysModuleTest", "test_43581", STRING_EQUAL)
        case_id = "test.test_sys.ActiveExceptionTests.test_sys_exception_no_exception"
        cases[case_id] = native_method(
            args, work, source, "ActiveExceptionTests", "test_sys_exception_no_exception",
            EXCEPTION_EQUAL)

        # Independent operational checks; these do not add to the upstream count.
        exercise = HERE / "active_exception.py"
        program = work / "active-exception"
        build(args, exercise, program)
        reference_result = subprocess.run([sys.executable, exercise], capture_output=True,
                                          check=True, timeout=15)
        native_result = subprocess.run([program], capture_output=True, check=True, timeout=15)
        if (native_result.stdout, native_result.stderr) != (reference_result.stdout, reference_result.stderr):
            raise AssertionError(("active exception context", native_result, reference_result))
        binary = work / "encoding"
        build(args, HERE / "encoding_driver.py", binary)
        calls = []

        def invoke(operation, *values):
            run = subprocess.run([binary, operation, *map(str, values)], capture_output=True,
                                 timeout=15)
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

        for name in ("test_getdefaultencoding", "test_getfilesystemencoding"):
            calls.clear()
            case = test_sys.SysModuleTest(name)
            with patch.object(test_sys, "sys", NativeSys()):
                result = unittest.TextTestRunner(verbosity=2).run(case)
            if not result.wasSuccessful() or result.skipped or not calls:
                raise AssertionError(case.id())
            cases[case.id()] = {"execution": "host-driven-native", "status": "pass",
                                "calls": list(calls)}

        # The default host locale can skip this case on the spelling of "UTF-8".
        # CPython's UTF-8 mode satisfies the unchanged decorator and oracle.
        case = test_sys.SysModuleTest("test_ioencoding_nonascii")
        child_sources = []

        def popen(argv, **kwargs):
            if len(argv) != 3 or argv[:2] != [sys.executable, "-c"]:
                raise ValueError(f"unexpected child: {argv!r}")
            script = work / "child.py"
            script.write_text(argv[2])
            child = work / "child"
            build(args, script, child)
            child_sources.append(hashlib.sha256(script.read_bytes()).hexdigest())
            return subprocess.Popen([child], **kwargs)

        with patch.object(test_sys, "subprocess", SimpleNamespace(Popen=popen, PIPE=subprocess.PIPE)):
            result = unittest.TextTestRunner(verbosity=2).run(case)
        if not result.wasSuccessful():
            raise AssertionError(case.id())
        if result.skipped:
            cases[case.id()] = {"status": "upstream-skip", "reason": result.skipped[0][1]}
        else:
            if len(child_sources) != 1:
                raise AssertionError(child_sources)
            cases[case.id()] = {"execution": "host-driven-native", "status": "pass",
                                "child_sha256": child_sources[0]}

    if not cases.keys() <= reference["reference"].keys() or commit != reference["cpython_commit"]:
        raise ValueError("case or source revision missing from the reference inventory")
    args.result.write_text(json.dumps({"cpython_commit": commit, "opt_level": args.opt_level,
                                       "denominator": reference["denominator"],
                                       "tests": cases}, indent=2) + "\n")


if __name__ == "__main__":
    main()
