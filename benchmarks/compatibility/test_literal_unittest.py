"""Exercise original identity, result lifecycle, fixture errors and interrupts."""
import sys
import tempfile
import unittest
from pathlib import Path

from collect_unittest_results import capture, parse
from generate_unittest_runner import generate


class LiteralRunnerTest(unittest.TestCase):
    def test_finite_names(self):
        source = generate(["example.one", "example.two"])
        self.assertIn("import example.one as _module_0", source)
        self.assertIn("loader.loadTestsFromModule(_module_1)", source)
        for modules in ([], ["x", "x"], ["x; print(1)"], ["x.class"]):
            with self.assertRaises(ValueError):
                generate(modules)

    def run_module(self, body, timeout=4):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "original.py").write_text(body)
            launcher = root / "launcher.py"
            launcher.write_text(generate(["original"]))
            code, _, stderr, timed = capture([sys.executable, "-B", str(launcher)], timeout)
            return code, parse(stderr, timed)

    def test_results_and_fixture_error(self):
        code, report = self.run_module('''import unittest
class A(unittest.TestCase):
    def test_error(self): raise ValueError("oops")
    @unittest.skip("reason")
    def test_skip(self): pass
    def test_subtest(self):
        with self.subTest(x=1): self.skipTest("sub")
class B(unittest.TestCase):
    @classmethod
    def setUpClass(cls): raise RuntimeError("fixture")
    def test_unstarted(self): pass
''')
        self.assertEqual(code, 1)
        self.assertEqual([(r["id"], r["outcome"]) for r in report["originals"]], [
            ("original.A.test_error", "error"), ("original.A.test_skip", "skip"),
            ("original.A.test_subtest", "pass"), ("original.B.test_unstarted", "not_run")])
        self.assertEqual(report["fixtures"][0]["outcome"], "error")
        self.assertEqual(len(report["subtest_skips"]), 1)

    def test_repeated_ids_and_expected_outcomes(self):
        code, report = self.run_module("""import unittest
class A(unittest.TestCase):
    @unittest.expectedFailure
    def test_expected(self): self.fail('expected')
    @unittest.expectedFailure
    def test_unexpected(self): pass
def load_tests(loader, tests, pattern):
    return unittest.TestSuite([A('test_expected'), A('test_expected'), A('test_unexpected')])
""")
        self.assertEqual(code, 1)
        self.assertEqual([(r['occurrence'], r['outcome']) for r in report['originals']],
                         [(1, 'expected_failure'), (2, 'expected_failure'), (1, 'unexpected_success')])

    def test_timeout_keeps_inflight_id(self):
        code, report = self.run_module('''import time, unittest
class A(unittest.TestCase):
    def test_a(self): time.sleep(20)
    def test_b(self): pass
''', timeout=0.8)
        self.assertIsNone(code)
        self.assertTrue(report["timed_out"])
        self.assertEqual([r["outcome"] for r in report["originals"]], ["timeout", "not_run"])


if __name__ == "__main__":
    unittest.main()
