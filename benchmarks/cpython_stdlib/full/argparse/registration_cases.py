"""Run a failed native registration at the point where the original method expects it."""

import ast
from functools import partial
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

from native_cases import Blocked, Driver, arguments, completed
from native_methods import Fixtures
from suite_inventory import cases, load_suite


def registration_test(node):
    if node is None or node.decorator_list or len(node.body) < 2:
        return False
    ctor, catch = node.body[:2]
    if not (isinstance(ctor, ast.Assign) and len(ctor.targets) == 1
            and isinstance(ctor.targets[0], ast.Name) and isinstance(ctor.value, ast.Call)
            and ast.unparse(ctor.value.func) == "argparse.ArgumentParser"
            and isinstance(catch, ast.With) and len(catch.items) == len(catch.body) == 1):
        return False
    context = catch.items[0].context_expr
    if not (isinstance(context, ast.Call) and ast.unparse(context.func) == "self.assertRaises"
            and len(context.args) == 1 and ast.unparse(context.args[0]) in {"TypeError", "ValueError"}):
        return False
    stmt = catch.body[0]
    add = ctor.targets[0].id + ".add_argument"
    if not (isinstance(stmt, ast.Expr) and isinstance(stmt.value, ast.Call) and ast.unparse(stmt.value.func) == add):
        return False
    allowed = {add, "argparse.ArgumentParser", "self.assertRaises", "self.assertIn", "self.assertEqual", "str"}
    calls = [ast.unparse(expr.func) for expr in ast.walk(node) if isinstance(expr, ast.Call)]
    return calls.count(add) == 1 and set(calls) <= allowed


class Registration:
    def __init__(self, driver, *args, **kwargs):
        self.driver = driver
        self.constructor = arguments(args, kwargs)

    def add_argument(self, *args, **kwargs):
        call = arguments(args, kwargs)
        source = (f"import argparse\n\ndef register():\n"
                  f"    parser = argparse.ArgumentParser({self.constructor})\n"
                  f"    try:\n        parser.add_argument({call})\n"
                  "    except TypeError as error:\n        print('TypeError')\n        print(repr(str(error)))\n"
                  "    except ValueError as error:\n        print('ValueError')\n        print(repr(str(error)))\n"
                  "    else:\n        print('added')\n\nregister()\n")
        binary, reason = self.driver.build(source)
        if reason:
            raise Blocked(reason)
        actual = completed([binary])
        expected = completed([sys.executable, "-B", binary.with_name("case.py")])
        if actual != expected:
            raise AssertionError(f"add_argument({call}): native={actual!r}, CPython={expected!r}")
        code, stdout, stderr = actual
        if code or stderr:
            raise AssertionError(f"unexpected registration process result: {actual!r}")
        self.driver.calls["add_argument"] += 1
        if stdout == b"added\n":
            return
        kind, message = stdout.decode().splitlines()
        raise {"TypeError": TypeError, "ValueError": ValueError}[kind](ast.literal_eval(message))


class RegistrationDriver(Driver):
    def parser_patch(self, case):
        from test import test_argparse as upstream
        return patch.object(upstream.argparse, "ArgumentParser", partial(Registration, self))


def measure(toolchain, module, reference, source, bodies):
    from test import test_argparse as upstream
    fixtures = Fixtures(source)
    for case in cases(load_suite(module)):
        cls, name = type(case), case.id()
        if bodies.get(name, {}).get("outcome") != "blocked" or cls.setUp is not upstream.TestCase.setUp:
            continue
        node = fixtures.methods.get((fixtures.owner(cls, case._testMethodName), case._testMethodName))
        if not registration_test(node):
            continue
        prior = bodies[name]["reason"]
        with tempfile.TemporaryDirectory(prefix="imre-argparse-registration-") as folder:
            driver = RegistrationDriver(toolchain, Path(folder), bodies)
            driver.outcomes(unittest.TestSuite([case]), {name: driver.execute}, reference)
            driver.build.cache_clear()
        proof = bodies[name].setdefault("evidence", {})
        proof["native_body_blocker"] = prior
        if bodies[name]["outcome"] == "pass":
            proof["transport"] = "native exception type and message from stdout"
