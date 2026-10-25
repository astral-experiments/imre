"""Compile the original default-encoding method in a native TestCase."""

import re
import subprocess
import sys

from suite_native import NativeCompileError, native_method, testcase_fixture


TYPE_ASSERTION = """
    def assertIsInstance(self, obj: object, cls, msg: str | None = None) -> None:
        if cls is str:
            if isinstance(obj, str):
                return
            expected = "<class 'str'>"
        elif cls is int:
            if isinstance(obj, int):
                return
            expected = "<class 'int'>"
        else:
            raise TypeError("encoding assertion fixture supports str and int")
        self.fail(self._formatMessage(msg, f"{obj!r} is not an instance of {expected}"))
"""

def assertion_controls(toolchain, folder, fixture):
    # These exercise failure paths which the original passing method never takes.
    controls = """
case = NativeCase()
case.assertIsInstance("ok", str)
case.assertIsInstance(42, int)
try:
    case.assertIsInstance(42, str)
except AssertionError as err:
    print(str(err))
try:
    case.assertIsInstance("hello", int, "wrong class")
except AssertionError as err:
    print(str(err))
try:
    case.assertRaises(TypeError, sys.getdefaultencoding)
except AssertionError:
    print("the zero-argument call returned")
"""
    folder.mkdir()
    native = toolchain.build((fixture + controls).encode(), folder)
    stock = "import sys\nfrom unittest import TestCase as NativeCase\n" + controls
    expected = subprocess.run([sys.executable, "-I", "-c", stock], capture_output=True, check=True, timeout=15)
    actual = subprocess.run([native], capture_output=True, check=True, timeout=15)
    if (actual.stdout, actual.stderr) != (expected.stdout, expected.stderr):
        raise AssertionError(("encoding assertion controls", actual, expected))


def select(toolchain, work, source, case_id, host):
    """Try the shared fixture first; retain measured host-driven operations on a compile blocker."""
    method = case_id.rsplit(".", 1)[1]
    base = "import sys\n" + testcase_fixture()
    operation = "sys.getdefaultencoding, including its one-argument TypeError"
    attempts = []
    native = None
    for label, extra in (("shared", ""), ("encoding assertion fixture", TYPE_ASSERTION)):
        folder = work / (method + ("-assertions" if extra else "-shared"))
        folder.mkdir()
        fixture = base + extra + "\n"
        try:
            native = native_method(toolchain, folder, source, "SysModuleTest", case_id, fixture,
                                   operation=("imre", operation))
        except NativeCompileError as exc:
            diagnostic = str(exc).replace(str(folder), "[case]").strip()
            match = re.search(r"\[case\]/case\.py:(\d+):(\d+): Imre: (.+)", diagnostic)
            line = (folder / "case.py").read_text().splitlines()[int(match[1]) - 1].strip() if match else None
            attempts.append({"fixture": label, "diagnostic": diagnostic, "statement": line})
            if extra or not (match and match[3] == "unknown class method" and line.startswith("self.assertIsInstance(")):
                break
        else:
            if extra:
                assertion_controls(toolchain, work / (method + "-controls"), fixture)
            break

    def observe(case, original):
        record = (native or host)(case, original)
        evidence = record["observation"]["evidence"]
        if attempts:
            evidence["whole_method_compile_attempts"] = attempts
        if native:
            evidence["assertion_fixture"] = label
        return record

    return observe
