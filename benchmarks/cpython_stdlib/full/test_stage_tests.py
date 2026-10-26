"""A modified or extra test file cannot enter the reference denominator unnoticed."""
from pathlib import Path
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parent))
from discover_staged import early_skips, suite_names
from run_staged_reference import compare_group
from stage_tests import stage, verify_prefix
from suite_inventory import passing_subtest_skips


class FixtureChecks(unittest.TestCase):
    def test_skipped_subtest_needs_a_passing_sibling_and_matching_junit_reason(self):
        class Parameters(unittest.TestCase):
            def test_parameters(self):
                for value in (0, 1, 2):
                    with self.subTest(value=value):
                        if value:
                            self.skipTest("this parameter")

            @unittest.skip("whole method")
            def test_whole(self):
                self.fail()

        count, skips, aliases = passing_subtest_skips(unittest.defaultTestLoader.loadTestsFromTestCase(Parameters))
        parent, whole = Parameters("test_parameters").id(), Parameters("test_whole").id()
        self.assertEqual(count, 2)
        self.assertEqual(skips, [[parent, {"value": 1}, "this parameter"], [parent, {"value": 2}, "this parameter"]])
        expected = {parent: ["pass"], whole: [{"skipped": "whole method"}]}
        root = ET.Element("testsuite")
        for name, reason in ((parent + " (value=1)", "this parameter"), (whole, "whole method")):
            case = ET.SubElement(root, "testcase", name=name, status="run", result="completed")
            ET.SubElement(case, "skipped").text = reason
        with tempfile.TemporaryDirectory() as temporary:
            xml = Path(temporary) / "suite.xml"
            ET.ElementTree(root).write(xml)
            result = compare_group(xml, expected, {}, 0, aliases)
            self.assertEqual(result["outcomes"], {"pass": 1, "skipped": 1})
            self.assertEqual(result["differences"], {})
            root[0][0].text = "a different reason"
            ET.ElementTree(root).write(xml)
            self.assertIn("incomplete", compare_group(xml, expected, {}, 0, aliases))

        class AllSkipped(unittest.TestCase):
            def test_parameters(self):
                with self.subTest(value=1):
                    self.skipTest("only parameter")

        with self.assertRaisesRegex(ValueError, "no successful sibling"):
            passing_subtest_skips(unittest.defaultTestLoader.loadTestsFromTestCase(AllSkipped))

    def test_incomplete_junit_cannot_supply_reference_counts(self):
        expected = {"test.suite.C.test_a": ["pass"], "test.suite.C.test_b": ["pass"]}
        with tempfile.TemporaryDirectory() as temporary:
            xml = Path(temporary) / "suite.xml"
            xml.write_text('<testsuite><testcase name="test.suite.C.test_a" status="run" '
                           'result="completed"/><testcase/></testsuite>')
            result = compare_group(xml, expected, {}, 0)
            self.assertIn("unnamed <testcase>", result["incomplete"])
            self.assertEqual(result["expected_instances"], 2)
            self.assertIsNone(result["instances"])
            self.assertEqual(result["outcomes"], {})

    def test_mixed_or_modified_test_installation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            count = stage(root)
            stdlib = root / "Lib"
            self.assertEqual(verify_prefix(stdlib), count)
            extra = stdlib / "test/test_unvendored.py"
            extra.touch()
            with self.assertRaisesRegex(ValueError, "unexpected installed fixture"):
                verify_prefix(stdlib)
            extra.unlink()
            (stdlib / "test/support/__init__.py").write_text("# changed upstream helper\n")
            with self.assertRaisesRegex(ValueError, "modified installed fixture"):
                verify_prefix(stdlib)

    def test_collections_owner_ledger_discovers_all_companion_suites(self):
        root = Path(__file__).resolve().parent
        entries = suite_names("collections", "test_collections", [root / "collections/reference-linux.json"])
        self.assertIn("test.test_collections", entries)
        self.assertIn("test.test_deque", entries)
        self.assertIn("test.test_free_threading.test_collections", entries)
        self.assertNotIn("collections", entries)

    def test_import_skip_does_not_mask_reference_passes(self):
        entries = [("test.suite.A.test_a", {"skipped": "GIL enabled"}), ("test.suite.B.test_b", "pass")]
        with self.assertRaisesRegex(ValueError, "test.suite.B.test_b"):
            early_skips("test.suite", "GIL enabled", entries)


if __name__ == "__main__":
    unittest.main()
