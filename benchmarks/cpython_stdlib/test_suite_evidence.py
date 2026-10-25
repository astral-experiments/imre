"""Regressions for per-occurrence attribution and rollup credit."""
from copy import deepcopy
import json
from pathlib import Path
import sys
from types import ModuleType
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from suite_evidence import ROLES, build_report, roles, validate_pair, validate_report
from suite_native import run_evidence, run_in_fixtures


def reference(*ids):
    outcomes = {}
    for name in ids:
        outcomes.setdefault(name, []).append("pass")
    return {"module": "test.example", "platform": "linux", "cpython_commit": "a" * 40,
            "denominator": len(ids), "reference": outcomes}


def observed(operation="imre", fixture="mixed", body="imre", assertions="imre"):
    return {"roles": roles(body=body, assertions=assertions, operation=operation, fixture=fixture),
            "evidence": {"operation": "example operation", "fixture": "example fixture"}}


def strict(case, original):
    original()
    return observed()


class ContractTests(unittest.TestCase):
    def test_duplicate_object_and_id(self):
        counts = []
        class Example(unittest.TestCase):
            def setUp(self):
                counts.append("setup")
            def testValue(self):
                counts.append("method")
        case = Example("testValue")
        key = case.id()
        ref = reference(key, key)
        def fail(case, original):
            self.fail("second occurrence")
        report = run_evidence(unittest.TestSuite([case, case]), {(key, 1): strict, (key, 2): fail}, ref, 0)
        first, second = report["records"]
        self.assertEqual([r["occurrence"] for r in (first, second)], [1, 2])
        self.assertEqual([r["classification"] for r in (first, second)], ["native-body-host-fixture", "failed"])
        self.assertIn("second occurrence", second["reason"])
        self.assertEqual(second["roles"]["operation"], "unknown")
        self.assertEqual(counts, ["setup", "method", "setup"])
        self.assertNotIn("testValue", vars(case))
        self.assertEqual(report["denominator"], 2)
        self.assertEqual(report["distinct_ids"], 1)

    def test_setup_and_teardown_are_attributed_to_their_occurrences(self):
        class BadSetup(unittest.TestCase):
            @classmethod
            def setUpClass(cls):
                raise RuntimeError("setup failed")
            def runTest(self):
                self.fail("must not start")
        class BadTeardown(unittest.TestCase):
            @classmethod
            def tearDownClass(cls):
                raise RuntimeError("teardown failed")
            def runTest(self):
                pass
        class Good(unittest.TestCase):
            def runTest(self):
                pass
        tests = [BadSetup(), BadSetup(), BadTeardown(), Good()]
        ids = [case.id() for case in tests]
        report = run_evidence(unittest.TestSuite(tests), dict.fromkeys(ids, strict), reference(*ids), 3)
        self.assertEqual([r["outcome"] for r in report["records"]], ["error", "error", "error", "pass"])
        first, _, third, _ = report["records"]
        self.assertEqual(first["roles"]["body"], "unobserved")
        self.assertIn("setup failed", first["reason"])
        self.assertEqual(third["roles"]["operation"], "imre")
        self.assertIn("teardown failed", third["reason"])

    def test_subtest_errors_and_unexpected_skips_are_not_passes(self):
        class Example(unittest.TestCase):
            def testBad(self):
                with self.subTest(value=10):
                    self.fail("bad value")
            def testSkip(self):
                self.skipTest("new skip")
        tests = [Example("testBad"), Example("testSkip")]
        ids = [case.id() for case in tests]
        report = run_evidence(unittest.TestSuite(tests), dict.fromkeys(ids, strict), reference(*ids), 0)
        bad, skip = report["records"]
        self.assertEqual(bad["classification"], "failed")
        self.assertIn("value=10", bad["evidence"]["details"][0]["id"])
        self.assertEqual(skip["classification"], "unexpected-skip")
        self.assertEqual(skip["reason"], "new skip")

    def test_module_setup_keeps_its_exact_variant_and_reason(self):
        module = ModuleType("_imre_evidence_fixture")
        def skip_module():
            raise unittest.SkipTest("native run's setup reason")
        module.setUpModule = skip_module
        sys.modules[module.__name__] = module
        self.addCleanup(sys.modules.pop, module.__name__)
        class Example(unittest.TestCase):
            def runTest(self):
                self.fail("module setup must prevent the method")
        Example.__module__ = module.__name__
        case = Example()
        key = case.id()
        ref = reference(key)
        ref["reference"][key] = {"skipped": "pinned reference reason"}
        exc = {"kind": "upstream-skip", "reason": "pinned reference reason", "condition": "fixture unavailable",
               "configuration": "module setup", "source": "test_example.py:2", "platform": "linux",
               "fixture_variant": "custom-backend"}
        report = run_evidence(unittest.TestSuite([case]), {key: strict}, ref, 0,
                              exceptions={(key, 1, 0): exc}, fixture_variant=lambda _: "custom-backend")
        row, = report["records"]
        self.assertEqual(row["classification"], "unexpected-skip")
        self.assertEqual(row["reference"], {"skipped": "pinned reference reason"})
        self.assertEqual(row["reason"], "native run's setup reason")
        self.assertEqual(row["roles"]["body"], "unobserved")
        self.assertEqual(row["evidence"]["details"][0]["fixture_event"], "setUpModule (_imre_evidence_fixture)")

    def test_all_role_categories_and_preflight_blockers(self):
        examples = [("fully-native", observed(fixture="none")), ("native-body-host-fixture", observed()),
                    ("host-driven-native-operation", observed(body="cpython", assertions="cpython")),
                    ("mixed", observed(operation="mixed")), ("host-operation", observed(operation="cpython")),
                    ("no-product-operation", observed(operation="none"))]
        ids = [name for name, _ in examples] + ["unattempted", "no compiler"]
        ref = reference(*ids)
        decisions = {(name, 1): dict(value, outcome="pass") for name, value in examples}
        decisions["no compiler", 1] = {"outcome": "blocked", "reason": "unsupported exception layout",
                                       "roles": dict.fromkeys(ROLES, "unobserved"), "evidence": {"stage": "compile"}}
        reports = [build_report(ref, level, [(name, "example.Fixture") for name in ids], decisions)
                   for level in (0, 3)]
        validate_pair(json.loads(json.dumps(reports)), ref)
        self.assertEqual([r["classification"] for r in reports[0]["records"]],
                         [name for name, _ in examples] + ["reference-only", "blocked"])
        with self.assertRaisesRegex(ValueError, "exactly one"):
            validate_pair(reports[:1], ref)
        forged = deepcopy(reports[0])
        forged["records"][4]["classification"] = "fully-native"
        with self.assertRaisesRegex(ValueError, "classification disagrees"):
            validate_report(forged, ref)
        forged = deepcopy(reports[0])
        forged["records"][0] = forged["records"][1]
        with self.assertRaisesRegex(ValueError, "duplicate"):
            validate_report(forged, ref)
        forged = deepcopy(reports[0])
        forged["records"][0]["evidence"]["opt_level"] = 3
        with self.assertRaisesRegex(ValueError, "build level"):
            validate_report(forged, ref)

    def test_exceptions_require_an_exact_occurrence_condition_and_review(self):
        key, variant = "test.example.A.testIt", "example.A"
        ref = reference(key, key)
        ref["reference"][key][0] = {"skipped": "no signal handler"}
        discovered = [(key, variant), (key, variant)]
        skip = {"kind": "upstream-skip", "reason": "no signal handler", "condition": "SIGINT is ignored",
                "configuration": "signal fixture", "source": "upstream/test.py:13",
                "platform": "linux", "fixture_variant": variant}
        exclusion = dict(skip, kind="reviewed-exclusion", reason="private allocator hook", condition="requires hook",
                         review="review decision for this ID", revisit="issue tracking this hook")
        decisions = {(key, 2): {"outcome": "excluded", "roles": dict.fromkeys(ROLES, "unobserved"),
                                "reason": exclusion["reason"]}}
        exceptions = {(key, 1, 0): skip, (key, 2, 0): exclusion}
        report = build_report(ref, 0, discovered, decisions, exceptions=exceptions)
        self.assertEqual(report["counts"], {"reference-skip": 1, "reviewed-exclusion": 1})
        for changed in ({(key, 1, 0): skip}, {(key, 1, 3): skip, (key, 2, 0): exclusion},
                        {(key, 1, 0): dict(skip, reason="changed reason"), (key, 2, 0): exclusion},
                        {(key, 1, 0): skip, (key, 2, 0): dict(exclusion, review="")}):
            with self.assertRaises(ValueError):
                build_report(ref, 0, discovered, decisions, exceptions=changed)

    def test_old_callback_remains_callable_and_is_not_inferred(self):
        class Example(unittest.TestCase):
            def runTest(self):
                pass
        case = Example()
        call = {case.id(): lambda case, method: {"execution": "native-body"}}
        evidence, failures, skips = run_in_fixtures(unittest.TestSuite([case]), call)
        self.assertEqual((failures, skips), ([], []))
        self.assertEqual(evidence[case.id()][0], {"execution": "native-body"})
        with self.assertRaisesRegex(ValueError, "legacy labels"):
            run_evidence(unittest.TestSuite([case]), call, reference(case.id()), 0)


if __name__ == "__main__":
    unittest.main()
