"""Compile both unchanged definitions that check the native filesystem encoding."""
import hashlib

from suite_native import method_bytes, native_method


def filesystem_method(toolchain, work, source, case_id):
    helper = method_bytes(source, "SysModuleTest", "check_fsencoding")
    fixture = """import codecs
import sys

class NativeCase:
    def assertIsNotNone(self, value: str | None) -> None:
        if value is None:
            raise AssertionError("unexpected None")

    def assertEqual(self, left: str, right: str | None) -> None:
        if right is None or left != right:
            raise AssertionError("filesystem encodings differ")
""" + helper.decode()
    native = native_method(
        toolchain, work, source, "SysModuleTest", case_id, fixture,
        operation=("imre", "sys.getfilesystemencoding, sys.platform and codecs.lookup in the original helper"),
    )

    def execute(case, original):
        record = native(case, original)
        digest = hashlib.sha256(helper).hexdigest()
        record["helper_sha256"] = digest
        record["observation"]["evidence"].update(helper="SysModuleTest.check_fsencoding", helper_sha256=digest)
        return record
    return execute
