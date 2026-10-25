"""Let unchanged source methods drive a native parser when their assertions cannot compile."""

import ast
from functools import partial
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

from native_cases import Driver, Parser
from native_methods import Fixtures
from suite_inventory import cases, load_suite


def transferable(node):
    # Only straight-line declarations, native calls, and host assertions on
    # transported values. Other argparse or fixture operations need their own adapter.
    syntax = (ast.FunctionDef, ast.arguments, ast.arg, ast.Assign, ast.Expr, ast.Call, ast.Name,
              ast.Load, ast.Store, ast.Attribute, ast.List, ast.Tuple, ast.Constant, ast.keyword,
              ast.UnaryOp, ast.USub, ast.UAdd)
    if node.decorator_list or any(not isinstance(n, syntax) for n in ast.walk(node)):
        return False
    if any(isinstance(n, ast.FunctionDef) and n is not node for n in ast.walk(node)):
        return False
    locals_ = {n.id for n in ast.walk(node) if isinstance(n, ast.Name) and isinstance(n.ctx, ast.Store)}
    parsers = {stmt.targets[0].id for stmt in node.body if isinstance(stmt, ast.Assign)
               and len(stmt.targets) == 1 and isinstance(stmt.targets[0], ast.Name)
               and isinstance(stmt.value, ast.Call) and ast.unparse(stmt.value.func) == "argparse.ArgumentParser"}
    if not parsers:
        return False
    native = False
    allowed = {"self", "argparse", "NS", "str", "float", "int"} | locals_
    for expr in ast.walk(node):
        if isinstance(expr, ast.Name) and expr.id not in allowed:
            return False
        if isinstance(expr, ast.Call):
            fn = expr.func
            if isinstance(fn, ast.Attribute) and isinstance(fn.value, ast.Name) and fn.value.id in parsers:
                if fn.attr not in {"add_argument", "parse_args", "parse_known_args"}:
                    return False
                native |= fn.attr.startswith("parse_")
            elif ast.unparse(fn) in {"argparse.ArgumentParser", "self.assertEqual", "NS"}:
                continue
            elif not (isinstance(fn, ast.Attribute) and fn.attr == "split"
                      and isinstance(fn.value, ast.Constant) and isinstance(fn.value.value, str)):
                return False
        if isinstance(expr, ast.Attribute) and isinstance(expr.value, ast.Name):
            if expr.value.id == "argparse" and expr.attr != "ArgumentParser":
                return False
            if expr.value.id == "self" and expr.attr != "assertEqual":
                return False
    return native


class PlainParser(Parser):
    def exited(self, code, stdout, stderr):
        sys.stdout.write(stdout.decode())
        sys.stderr.write(stderr.decode())
        raise SystemExit(code)


class DirectDriver(Driver):
    def parser_patch(self, case):
        from test import test_argparse as upstream
        return patch.object(upstream.argparse, "ArgumentParser", partial(PlainParser, self))


def measure(toolchain, module, reference, source, bodies):
    from test import test_argparse as upstream
    fixtures = Fixtures(source)
    classes = {}
    for case in cases(load_suite(module)):
        cls = type(case)
        if bodies.get(case.id(), {}).get("outcome") != "blocked" or cls.setUp is not upstream.TestCase.setUp:
            continue
        node = fixtures.methods.get((fixtures.owner(cls, case._testMethodName), case._testMethodName))
        if node and transferable(node):
            classes.setdefault(cls, []).append(case)
    attempted = {}
    for members in classes.values():
        with tempfile.TemporaryDirectory(prefix="imre-argparse-direct-") as work:
            driver = DirectDriver(toolchain, Path(work), attempted)
            driver.outcomes(unittest.TestSuite(members), {case.id(): driver.execute for case in members}, reference)
            driver.build.cache_clear()
    for name, observation in attempted.items():
        proof = observation.setdefault("evidence", {})
        proof["native_body_blocker"] = bodies[name]["reason"]
        bodies[name] = observation
    return attempted
