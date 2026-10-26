"""Run the original mutually exclusive group routines with native parser operations."""

from pathlib import Path
import tempfile
import unittest

from subparser_cases import ErrorParserDriver
from suite_inventory import cases, load_suite


def selected(case, upstream):
    cls, method = type(case), case._testMethodName
    if issubclass(cls, upstream.TestMutuallyExclusiveGroupErrors):
        # These methods construct the parsers they exercise. The parent variant
        # inherits them unchanged, with its own original test ID.
        return method in {"test_help", "test_optional_order", "test_usage_empty_group"}
    # The parent variant requires an existing parser as input to a new parser.
    return (isinstance(case, upstream.MEMixin) and not isinstance(case, upstream.MEPBase)
            and method.startswith("test_") and method in upstream.MEMixin.__dict__)


def measure(toolchain, module, reference, bodies):
    from test import test_argparse as upstream
    members = [case for case in cases(load_suite(module))
               if bodies.get(case.id(), {}).get("outcome") == "blocked" and selected(case, upstream)]
    with tempfile.TemporaryDirectory(prefix="imre-argparse-mutex-") as directory:
        attempted = {}
        driver = ErrorParserDriver(toolchain, Path(directory), attempted)
        driver.outcomes(unittest.TestSuite(members), {case.id(): driver.execute for case in members}, reference)
        driver.build.cache_clear()
    for name, result in attempted.items():
        proof = result.setdefault("evidence", {})
        proof["native_body_blocker"] = bodies[name]["reason"]
        bodies[name] = result
