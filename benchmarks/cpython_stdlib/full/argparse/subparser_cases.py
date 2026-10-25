"""Run original subparser methods that create and exercise a parser in the test body."""

from functools import partial
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from help_cases import HelpParser
from native_cases import Driver
from suite_inventory import cases, load_suite


# The original setUp builds two unused host parsers. These methods construct
# their own parser; every parse/format call goes through the compiled program.
METHODS = {
    "test_dest", "test_optional_subparsers", "test_required_subparsers_default",
    "test_required_subparsers_no_destination_error", "test_required_subparsers_via_attribute",
    "test_required_subparsers_via_kwarg", "test_subparser_title_help",
}


class SubparserDriver(Driver):
    def parser_patch(self, case):
        from test import test_argparse as upstream
        return patch.object(upstream, "ErrorRaisingArgumentParser", partial(HelpParser, self))


def measure(toolchain, module, reference, bodies):
    from test import test_argparse as upstream
    members = [case for case in cases(load_suite(module)) if type(case) is upstream.TestAddSubparsers
               and case._testMethodName in METHODS and bodies[case.id()]["outcome"] == "blocked"]
    with tempfile.TemporaryDirectory(prefix="imre-argparse-subparsers-") as work:
        attempted = {}
        driver = SubparserDriver(toolchain, Path(work), attempted)
        driver.outcomes(unittest.TestSuite(members), {case.id(): driver.execute for case in members}, reference)
        driver.build.cache_clear()
    for name, observation in attempted.items():
        evidence = observation.setdefault("evidence", {})
        evidence["native_body_blocker"] = bodies[name]["reason"]
        evidence["host_setup"] = "Original setUp constructs separate parsers; the method creates its parser."
        bodies[name] = observation
