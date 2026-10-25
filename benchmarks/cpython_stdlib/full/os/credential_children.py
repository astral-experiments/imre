"""Run complete credential methods, recording where their exact -c payload runs."""
import argparse
import ast
from contextlib import chdir
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from native_descriptors import HERE, descriptor_suite, verify_sources
from method_preflight import methods
from credential_boundaries import observe_boundaries
from credential_errors import run_errors
from suite_native import NativeCompileError, Toolchain, testcase_fixture

POSIX_ASSERTIONS = {"test_getresuid": 4, "test_getresgid": 4, "test_setresuid": 2, "test_setresgid": 2}
POSIX_CASE = """import posix

class Case:
    checks: int

    def __init__(self) -> None:
        self.checks = 0

    def assertEqual(self, actual: int, expected: int) -> None:
        assert actual == expected
        self.checks += 1

    def assertGreaterEqual(self, actual: int, expected: int) -> None:
        assert actual >= expected
        self.checks += 1

    def assertIsNone(self, actual: None) -> None:
        assert actual is None
        self.checks += 1

"""


def execute_saved_ids(case, tools, root):
    method = case._testMethodName
    body, _ = methods(HERE / "upstream/Lib/test/test_posix.py")[f"PosixTester.{method}"]
    directory = root / method
    directory.mkdir()
    source = POSIX_CASE.encode() + body + f"\ncase = Case()\ncase.{method}()\nprint(case.checks)\n".encode()
    binary = tools.build(source, directory)
    ref = subprocess.run([sys.executable, "-IB", directory / "case.py"], cwd=directory,
                         start_new_session=True, capture_output=True, timeout=30, check=True)
    run = subprocess.run([binary], cwd=directory, start_new_session=True,
                         capture_output=True, timeout=30, check=True)
    expected = f"{POSIX_ASSERTIONS[method]}\n".encode()
    if ref.stdout != expected or run.stdout != expected or ref.stderr or run.stderr:
        raise AssertionError((case.id(), ref.stdout, run.stdout, ref.stderr, run.stderr))
    return {"status": "native_body_host_fixture", "assertions": POSIX_ASSERTIONS[method],
            "method_sha256": hashlib.sha256(body).hexdigest(),
            "host_fixtures": ["PosixTester.setUp and cleanups; upstream feature guard; test_posix.tearDownModule",
                              "fresh process and temporary directory for each saved-ID operation"]}


def verify_converters(tools, root):
    source = (HERE / "credential_arguments.py").read_bytes()
    directory = root / "converter"
    directory.mkdir()
    binary = tools.build(source, directory)
    ref = subprocess.run([sys.executable, "-IB", directory / "case.py"], cwd=directory,
                         start_new_session=True, capture_output=True, timeout=30, check=True)
    run = subprocess.run([binary], cwd=directory, start_new_session=True,
                         capture_output=True, timeout=30, check=True)
    if run.stdout != ref.stdout or run.stderr != ref.stderr:
        raise AssertionError((run.stdout, ref.stdout, run.stderr, ref.stderr))
    return {"source_sha256": hashlib.sha256(source).hexdigest(), "output_sha256": hashlib.sha256(ref.stdout).hexdigest(),
            "matching_lines": len(ref.stdout.splitlines())}


def execute(case, tools, root):
    method = case._testMethodName
    operation = method.removeprefix("test_").removesuffix("_neg1")
    if operation not in {"setregid", "setreuid"}:
        raise ValueError(operation)
    body, _ = methods(HERE / "upstream/Lib/test/test_os.py")[f"PosixUidGidTests.{method}"]
    definition = ast.parse(b"class P:\n" + body).body[0].body[0]
    args = definition.body[0].value.args[0].elts
    payload = args[2].value
    # Only these two no-change requests are approved for this fixture. The outer
    # method and inner program both come from the unchanged, blob-verified source.
    if (ast.unparse(args[0]), args[1].value, payload) != (
            "sys.executable", "-c", f"import os,sys;os.{operation}(-1,-1);sys.exit(0)"):
        raise ValueError(f"unexpected credential child in {case.id()}")
    directory = root / method
    directory.mkdir()
    child_dir = directory / "child"
    child_dir.mkdir()
    try:
        interpreter = tools.build(payload.encode() + b"\n", child_dir)
        location, reason = "imre", None
    except NativeCompileError as exc:
        interpreter = Path(sys.executable)
        location = "cpython"
        reason = str(exc).replace(str(child_dir) + "/", "").strip().splitlines()[-1]

    # The witness checks the child request and execs the selected interpreter.
    # A native parent or a successful check_call alone never proves a native OS call.
    witness = directory / "witness"
    launcher = directory / "interpreter"
    launcher.write_text("#!/bin/sh\n"
                        + f'[ "$#" -eq 2 ] && [ "$1" = -c ] && [ "$2" = {shlex.quote(payload)} ] || exit 93\n'
                        + f'printf "%s\\n" "$2" >> {shlex.quote(str(witness))}\n'
                        + f'exec {shlex.quote(str(interpreter))} "$@"\n')
    launcher.chmod(0o700)
    entry = ("import subprocess\nimport sys\n" + testcase_fixture() + "\n").encode() + body
    binary = tools.build(entry + f"\nNativeCase().{method}()\n".encode(), directory)
    def identity():
        return os.getuid(), os.geteuid(), os.getgid(), os.getegid()

    before = identity()
    subprocess.run([binary], cwd=directory, env={**os.environ, "IMRE_PYTHON": str(launcher)},
                   start_new_session=True, timeout=30, check=True)
    if witness.read_text().splitlines() != [payload] or before != identity():
        raise AssertionError(f"child arguments or fixture credentials changed: {case.id()}")
    return {"status": "native_body_host_operation" if location == "cpython" else "native_body_host_fixture",
            "method_sha256": hashlib.sha256(body).hexdigest(), "assertions": 0,
            "host_fixtures": ["original unittest feature and subprocess guards; test_os.tearDownModule",
                              "temporary directory, isolated child and argv witness"],
            "child_operation": {"operation": f"os.{operation}(-1, -1)", "interpreter": location,
                                "source": payload, "native_compile_blocker": reason}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("imre", "runtime", "clang", "cpython-source", "result"):
        parser.add_argument("--" + flag, type=Path, required=True)
    parser.add_argument("--opt-level", choices=(0, 3), type=int, required=True)
    args = parser.parse_args()
    modules, pin = verify_sources(args.cpython_source)
    tools = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), args.opt_level)
    result = args.result.resolve()
    with tempfile.TemporaryDirectory() as temp, chdir(temp):
        root = Path(temp)
        selected = {"test.test_os.PosixUidGidTests.test_" + op + "_neg1":
                    lambda case, original: execute(case, tools, root) for op in ("setregid", "setreuid")}
        rows = descriptor_suite(modules["test_os"], selected)
        saved = {"test.test_posix.PosixTester." + name:
                 (lambda case, original: execute_saved_ids(case, tools, root)) for name in POSIX_ASSERTIONS}
        rows.update(descriptor_suite(modules["test_posix"], saved))
        rows.update(run_errors(modules, tools, root))
        converter = verify_converters(tools, root)
        boundaries = observe_boundaries(tools, root)
        # The unchanged decorators decide availability before any callback is
        # entered. Expose the reason without feeding an unexecuted ID to the ledger.
        skipped = {name: row["reference_skip"] for name, row in rows.items() if "reference_skip" in row}
        rows = {name: row for name, row in rows.items() if name not in skipped}
        if any(row.get("status") not in {"native_body_host_operation", "native_body_host_fixture", "host_driven_native"}
               for row in rows.values()):
            raise ValueError(rows)
    result.write_text(json.dumps({"cpython_commit": pin, "opt_level": args.opt_level, "cases": rows,
                                  "reference_skips": skipped, "converter": converter,
                                  "supplemental_boundaries": boundaries}, indent=2) + "\n")


if __name__ == "__main__":
    main()
