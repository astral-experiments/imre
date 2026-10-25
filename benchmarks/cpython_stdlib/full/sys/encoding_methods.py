"""Compile the original encoding methods and their helper in a native TestCase."""

import hashlib
import re
import subprocess
import sys

from suite_native import NativeCompileError, method_bytes, native_method, testcase_fixture


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

FILESYSTEM_ASSERTIONS = """
    def assertIsNotNone(self, obj: object, msg: str | None = None) -> None:
        if obj is None:
            self.fail(self._formatMessage(msg, "unexpectedly None"))

    def assertEqual(self, first: str | None, second: str | None) -> None:
        if first != second:
            raise AssertionError(f"{first!r} != {second!r}")
"""


def assertion_controls(toolchain, folder, fixture, filesystem):
    # These exercise failure paths which the original passing method never takes.
    controls = """
case = NativeCase()
case.assertIsNotNone("utf-8")
case.assertEqual("utf-8", "utf-8")
try:
    case.assertIsNotNone(None)
except AssertionError as err:
    print(str(err))
try:
    case.assertEqual("ascii", None)
except AssertionError as err:
    print(str(err))
""" if filesystem else """
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
    filesystem = method == "test_getfilesystemencoding"
    helper = method_bytes(source, "SysModuleTest", "check_fsencoding") if filesystem else b""
    base = "import sys\n" + ("import codecs\n" if filesystem else "") + testcase_fixture()
    operation = ("sys.getfilesystemencoding, sys.platform, codecs.lookup" if filesystem else
                 "sys.getdefaultencoding, including its one-argument TypeError")
    attempts = []
    native = None
    assertion = FILESYSTEM_ASSERTIONS if filesystem else TYPE_ASSERTION
    for label, extra in (("shared", ""), ("encoding assertion fixture", assertion)):
        folder = work / (method + ("-assertions" if extra else "-shared"))
        folder.mkdir()
        fixture = base + extra + "\n" + helper.decode()
        try:
            native = native_method(toolchain, folder, source, "SysModuleTest", case_id, fixture,
                                   operation=("imre", operation))
        except NativeCompileError as exc:
            diagnostic = str(exc).replace(str(folder), "[case]").strip()
            match = re.search(r"\[case\]/case\.py:(\d+):(\d+): Imre: (.+)", diagnostic)
            line = (folder / "case.py").read_text().splitlines()[int(match[1]) - 1].strip() if match else None
            attempts.append({"fixture": label, "diagnostic": diagnostic, "statement": line})
            absent = "self.assertIsNotNone(" if filesystem else "self.assertIsInstance("
            if extra or not (match and match[3] == "unknown class method" and line.startswith(absent)):
                break
        else:
            if extra:
                assertion_controls(toolchain, work / (method + "-controls"), fixture, filesystem)
            break

    def observe(case, original):
        record = (native or host)(case, original)
        evidence = record["observation"]["evidence"]
        if helper:
            evidence["original_helper_sha256"] = hashlib.sha256(helper).hexdigest()
        if attempts:
            evidence["whole_method_compile_attempts"] = attempts
        if native:
            evidence["assertion_fixture"] = label
        return record

    return observe
