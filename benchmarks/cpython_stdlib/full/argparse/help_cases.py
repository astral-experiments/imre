"""Run the complete generated help and usage routines under their original fixtures."""

from functools import partial
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

from native_cases import Blocked, Driver, Group, Parser, arguments, completed
from suite_inventory import cases, load_suite


class Commands(Group):
    def add_parser(self, *args, **kwargs):
        child = f"child{len(self.declarations)}"
        self.declarations.append(f"{child} = {self.name}.add_parser({arguments(args, kwargs)})")
        return Group(self.declarations, child)

    @property
    def required(self):
        raise Blocked("reading the native subparser action is unsupported")

    @required.setter
    def required(self, value):
        self.declarations.append(f"{self.name}.required = {value!r}")


class HelpParser(Parser):
    def add_subparsers(self, *args, **kwargs):
        self.declarations.append(f"commands = parser.add_subparsers({arguments(args, kwargs)})")
        return Commands(self.declarations, "commands")

    def run(self, method):
        declarations = "\n    ".join(self.declarations)
        observe = "sys.stdout.write(make())" if method.startswith("format_") else "make()"
        source = (f"import argparse\nimport sys\n\ndef make():\n    {declarations}\n"
                  f"    return parser.{method}()\n\n{observe}\n")
        binary, reason = self.driver.build(source)
        if reason:
            raise Blocked(reason)
        actual = completed([binary])
        expected = completed([sys.executable, "-B", binary.with_name("case.py")])
        if actual != expected or actual[0] != 0 or actual[2]:
            raise AssertionError(f"{method}: native={actual!r}, CPython={expected!r}")
        self.driver.calls[method] += 1
        return actual[1].decode()

    def format_help(self):
        return self.run("format_help")

    def format_usage(self):
        return self.run("format_usage")

    def print_help(self, file=None):
        self.printed("print_help", file)

    def print_usage(self, file=None):
        self.printed("print_usage", file)

    def printed(self, method, file):
        if file is not None:
            raise Blocked("original StdIOBuffer is a TextIOWrapper over BytesIO; native stream transfer unavailable")
        sys.stdout.write(self.run(method))


class HelpDriver(Driver):
    def parser_patch(self, case):
        from test import test_argparse as upstream
        return patch.object(upstream.argparse, "ArgumentParser", partial(HelpParser, self))


def measure(toolchain, module, reference, observations):
    from test import test_argparse as upstream
    classes = {}
    for case in cases(load_suite(module)):
        if isinstance(type(case), upstream.TestHelpFormattingMetaclass):
            classes.setdefault(type(case), []).append(case)
    for members in classes.values():
        with tempfile.TemporaryDirectory(prefix="imre-argparse-help-") as work:
            driver = HelpDriver(toolchain, Path(work), observations)
            driver.outcomes(unittest.TestSuite(members), {c.id(): driver.execute for c in members}, reference)
            driver.build.cache_clear()
