"""Run the complete recursion-limit methods under their original CPython fixtures."""
import ast
import hashlib
from pathlib import Path
import re
import select
import subprocess
import sys
from types import SimpleNamespace
from unittest.mock import patch

from suite_evidence import roles
from suite_native import NativeCompileError, method_bytes, native_method, testcase_fixture

HERE = Path(__file__).resolve().parent
PREFIX = "test.test_sys.SysModuleTest."
METHODS = ("test_getrecursionlimit", "test_setrecursionlimit")


def getter_with_type_assertion(toolchain, folder, source, diagnostic):
    match = re.search(r"case\.py:(\d+):(\d+): Imre: unknown class method\s*$", diagnostic)
    if not match:
        return None
    line, column = map(int, match.groups())
    if not (folder / "case.py").read_text().splitlines()[line - 1][column - 1:].startswith("self.assertIsInstance("):
        return None
    fixture = "import sys\n" + testcase_fixture() + """
    def assertIsInstance(self, value: object, cls) -> None:
        if cls is int:
            if isinstance(value, int):
                return
        elif cls is str:
            if isinstance(value, str):
                return
        else:
            raise ValueError("unsupported fixture class")
        raise AssertionError("value has the wrong class")
"""
    controls = folder / "type-controls"
    controls.mkdir()
    program = fixture + """
case = NativeCase()
try:
    case.assertIsInstance("hello", int)
except AssertionError:
    print("wrong value")
try:
    case.assertIsInstance(sys.getrecursionlimit(), str)
except AssertionError:
    print("wrong class")
"""
    binary = toolchain.build(program.encode(), controls)
    result = subprocess.run([binary], capture_output=True, check=True, timeout=15)
    if result.stdout != b"wrong value\nwrong class\n" or result.stderr:
        raise AssertionError((result.stdout, result.stderr))
    run = native_method(toolchain, folder, source, "SysModuleTest", PREFIX + "test_getrecursionlimit", fixture,
                        operation=("imre", "read the native recursion limit and reject an extra argument"))

    def execute(case, original):
        record = run(case, original)
        record["observation"]["evidence"].update(
            shared_assertion_blocker="unittest.TestCase.assertIsInstance: unknown class method",
            assertions="both swapped classes raise AssertionError; inherited native assertGreater and callable assertRaises",
        )
        return record

    return execute


def observer(module, command, digest, level, diagnostic):
    def execute(case, original):
        calls = []
        with subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, text=True, encoding="utf-8") as child:
            def request(op, *args):
                if op == "getrecursionlimit" and args in ((), (42,)):
                    lines = ["get-extra" if args else "get"]
                elif op == "setrecursionlimit" and not args:
                    lines = ["set-missing"]
                elif op == "setrecursionlimit" and len(args) == 1 and type(args[0]) is int:
                    lines = ["set-int", str(args[0])]
                else:
                    raise AssertionError((op, args))
                child.stdin.write("\n".join(lines) + "\n")
                child.stdin.flush()
                if not select.select([child.stdout], [], [], 15)[0]:
                    raise TimeoutError(op)
                line = child.stdout.readline()
                if not line.endswith("\n"):
                    raise AssertionError((op, line))
                reply = line.rstrip("\n")
                event = {"operation": op, "args": list(args), "reply": reply}
                calls.append(event)
                kind, _, message = reply.partition(": ")
                errors = {"TypeError": TypeError, "ValueError": ValueError, "RecursionError": RecursionError}
                if kind in errors:
                    raise errors[kind](message)
                return ast.literal_eval(reply)

            api = SimpleNamespace(getrecursionlimit=lambda *a: request("getrecursionlimit", *a),
                                  setrecursionlimit=lambda *a: request("setrecursionlimit", *a))
            try:
                with patch.object(module, "sys", api):
                    original()
                out, err = child.communicate("quit\n", timeout=15)
                if child.returncode or out or err:
                    raise AssertionError((child.returncode, out, err))
            finally:
                if child.poll() is None:
                    child.kill()
        if not calls:
            raise AssertionError(f"{case.id()} made no native calls")
        return {
            "execution": "host-driven-native", "body": "CPython", "assertions": "CPython",
            "fixture_execution": "original CPython SysModuleTest lifecycle", "calls": calls,
            "observation": {
                "roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                "evidence": {"operation": "native limit reads, mutations, and argument errors in one child",
                             "assertions": "unchanged CPython type, value, and exception assertions",
                             "fixture": "original SysModuleTest lifecycle and reap_children teardown",
                             "native_calls": len(calls), "calls": calls, "method_sha256": digest,
                             "opt_level": level, "whole_native_body": diagnostic},
            },
        }
    return execute


def cases(module, toolchain, work, source):
    sessions = {}
    driver = None
    for method in METHODS:
        folder = work / method
        folder.mkdir()
        case_id = PREFIX + method
        try:
            sessions[case_id] = native_method(
                toolchain, folder, source, "SysModuleTest", case_id, "import sys\n" + testcase_fixture(),
                operation=("imre", "read and configure the native recursion limit"))
        except NativeCompileError as exc:
            if method == "test_getrecursionlimit":
                getter = getter_with_type_assertion(toolchain, folder, source, str(exc))
                if getter is not None:
                    sessions[case_id] = getter
                    continue
            diagnostic = {"stage": "compile", "diagnostic": str(exc).replace(str(folder), "[case]").strip()}
            if driver is None:
                driver_dir = work / "recursion-session"
                driver_dir.mkdir()
                driver = toolchain.build((HERE / "recursion_driver.py").read_bytes(), driver_dir)
            body = method_bytes(source, "SysModuleTest", method)
            sessions[case_id] = observer(module, [driver], hashlib.sha256(body).hexdigest(),
                                         toolchain.level, diagnostic)
    control = HERE / "recursion_controls.py"
    folder = work / "recursion-controls"
    folder.mkdir()
    native = toolchain.build(control.read_bytes(), folder)
    expected = subprocess.run([sys.executable, "-B", control], capture_output=True, check=True, timeout=15)
    actual = subprocess.run([native], capture_output=True, check=True, timeout=15)
    if (actual.stdout, actual.stderr) != (expected.stdout, expected.stderr):
        raise AssertionError(("recursion controls", actual.stdout, actual.stderr, expected.stdout, expected.stderr))
    return sessions
