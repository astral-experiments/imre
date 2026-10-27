"""Exercise native method failures outside the pinned sys denominator."""
from pathlib import Path
import unittest

from occurrences import testcase_import_blocker
from suite_native import native_method, run_in_fixtures, testcase_fixture


class FailingBody(unittest.TestCase):
    def test_assertion(self):
        self.assertEqual(1, 2)

    def test_missing_module(self):
        raise ModuleNotFoundError("No module named 'unittest'")


def check_body_failures(toolchain, root):
    fixture = testcase_fixture()
    for method, diagnostic in (
        ("test_assertion", "AssertionError: 1 != 2"),
        ("test_missing_module", "ModuleNotFoundError: No module named 'unittest'"),
    ):
        case = FailingBody(method)
        folder = root / method
        folder.mkdir()
        children = []
        execute = native_method(toolchain, folder, Path(__file__), "FailingBody", case.id(),
                                fixture, on_child_error=children.append)
        observed, failures, skips = run_in_fixtures(unittest.TestSuite([case]), {case.id(): execute})
        if observed or skips or len(failures) != 1 or len(children) != 1:
            raise AssertionError((case.id(), observed, failures, skips, children))
        child, = children
        if (child.returncode != 1 or child.stdout or child.stderr.splitlines()[-1:] != [diagnostic]
                or testcase_import_blocker(child, folder.resolve() / "case.py", fixture) is not None):
            raise AssertionError((case.id(), child))
