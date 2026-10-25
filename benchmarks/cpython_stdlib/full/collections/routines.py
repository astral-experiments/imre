"""Probe complete original collections methods, preserving fixture and compiler diagnostics.

A successful probe does not certify that its assertions exercise a native collections
operation. The module's audited gates provide that evidence separately.
"""

import argparse
import ast
import doctest
from functools import cache
import inspect
import json
from pathlib import Path
import re
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE.parents[1]), str(HERE)]
from suite_inventory import cases, load_suite, verify_files
from suite_native import Toolchain, native_method, run_in_fixtures
from native_counter import assertion_fixture
from chainmap_methods import OPERATIONS as CHAINMAP_OPERATIONS, fixture as chainmap_fixture
from deque_methods import METHODS as DEQUE_METHODS, method_fixture as deque_fixture


COUNTER_OPERATIONS = {
    "test_unary": "Native unary Counter operations and conversion to plain dictionaries",
    "test_total": "Counter construction and total()",
    "test_invariant_for_the_in_operator": "Counter construction, key iteration, subscription and membership",
    "test_eq": "Counter equality and inequality with missing zero counts",
    "test_le": "Counter multiset inclusion with missing zero counts",
    "test_lt": "Counter strict multiset inclusion with missing zero counts",
    "test_ge": "Counter multiset containment with missing zero counts",
    "test_gt": "Counter strict multiset containment with missing zero counts",
}

# For the original unary test both operands are plain dict[str, int]. Compare
# stored keys (including absence) and their counts without depending on order.
UNARY_ASSERTIONS = """from collections import Counter

class NativeCase:
    def assertEqual(self, left: dict[str, int], right: dict[str, int]) -> None:
        if len(left) != len(right):
            raise AssertionError("different lengths")
        for key in left:
            if key not in right or left[key] != right[key]:
                raise AssertionError("different entries")
"""

# The C OrderedDict fixture supplies its concrete constructor as a callable.
# It never asks for the class identity or subclasses; those variants are separate.
ORDERED_SETITEM = """from collections import OrderedDict as ConcreteOrderedDict

class NativeCase:
    def OrderedDict(self, pairs: list[tuple[str, int]]) -> ConcreteOrderedDict[str, int]:
        return ConcreteOrderedDict(pairs)

    def assertEqual(self, left: list[tuple[str, int]], right: list[tuple[str, int]]) -> None:
        if left != right:
            raise AssertionError("different ordered entries")
"""
EMPTY = {f"test.test_deque.TestSequence.{method}" for method in
         ("test_getitem", "test_getslice", "test_subscript")}
DEQUE_OPERATIONS = {
    "test_clear": "Construct a deque, clear it twice, and inspect its native length and iteration",
    "test_maxlen_zero": "Consume native range iterators in bounded deque construction, extend, and extendleft",
}


@cache
def parsed(path):
    source = path.read_text()
    tree = ast.parse(source)
    methods = {(cls.name, method.name): method for cls in tree.body
               if isinstance(cls, ast.ClassDef) for method in cls.body
               if isinstance(method, ast.FunctionDef)}
    return source, tree.body, methods


def native_fixture(path, owner, method, case):
    """Supply original global imports/data used by this method and known factory bindings."""
    source, definitions, methods = parsed(path)
    node = methods[owner, method]
    names = {n.id for n in ast.walk(node) if isinstance(n, ast.Name)}
    members = {n.attr for n in ast.walk(node) if isinstance(n, ast.Attribute)
               and isinstance(n.value, ast.Name) and n.value.id == "self"}
    imports = []
    bindings = []
    for name in ("type2test", "OrderedDict"):
        value = getattr(type(case), name, None)
        if name in members and isinstance(value, type):
            module = value.__module__
            if module == "collections":
                imports.append(f"from collections import {value.__name__}")
            if module in ("collections", "builtins"):
                bindings.append(f"    {name} = {value.__name__}")
    for definition in definitions:
        if isinstance(definition, (ast.Import, ast.ImportFrom)):
            aliases = [alias for alias in definition.names
                       if (alias.asname or alias.name.split(".")[0]) in names]
            if aliases:
                if isinstance(definition, ast.Import):
                    selected = ast.Import(names=aliases)
                else:
                    selected = ast.ImportFrom(module=definition.module, names=aliases,
                                              level=definition.level)
                imports.append(ast.unparse(selected))
        # These are the original module-level data used by the queue and
        # namedtuple methods. Preserve their expressions exactly.
        if isinstance(definition, ast.Assign) and any(isinstance(t, ast.Name)
                and t.id in names & {"BIG", "TestNT"} for t in definition.targets):
            if any(isinstance(t, ast.Name) and t.id == "TestNT" for t in definition.targets):
                imports.append("from collections import namedtuple")
            imports.append(ast.get_source_segment(source, definition))
    imports.append("import unittest")
    prefix = "\n".join(dict.fromkeys(imports)) + "\n\nclass NativeCase(unittest.TestCase):\n"
    if bindings:
        prefix += "\n".join(bindings) + "\n"
    return node, prefix


def failure(exc, directory, node, fixture):
    message = str(exc).strip().splitlines()[-1]
    match = re.match(re.escape(str(directory / "case.py")) + r":(\d+):(\d+): (.*)", message)
    if match:
        line, column, message = match.groups()
        offset = int(line) - (fixture.count("\n") + 2)
        location = (f"upstream:{node.lineno + offset}:{column}" if offset >= 0
                    else f"fixture:{line}:{column}")
        return {"result": "compile-blocked", "location": location,
                "reason": message.replace(str(directory), "<work>")}
    return {"result": "fixture-blocked" if isinstance(exc, ValueError) else "compile-blocked",
            "reason": message.replace(str(directory), "<work>")}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + option, type=Path, required=True)
    parser.add_argument("--opt-level", choices=(0, 3), type=int, required=True)
    parser.add_argument("--compiler-revision", required=True)
    parser.add_argument("--class", dest="class_name", help="probe only this class")
    parser.add_argument("--module", help="probe one complete upstream module in the manifest")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use a test-enabled CPython 3.14.5")
    import test

    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    if args.module and args.module not in manifest["modules"]:
        parser.error("--module must name one of the original suites in UPSTREAM.json")
    reference = json.loads((HERE / "reference-linux.json").read_text())["reference"]
    upstream = Path(test.__file__).resolve().parents[2]
    verify_files(HERE / "upstream", manifest["files"])
    verify_files(upstream, manifest["files"])
    tools = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    results = {}
    for name in manifest["modules"]:
        if args.module and name != args.module:
            continue
        try:
            suite = load_suite(name)
        except unittest.SkipTest:
            for case_id, outcome in reference.items():
                if case_id.startswith(name + ".") and isinstance(outcome, dict) and not args.class_name:
                    results[case_id] = {"result": "reference-skip", "reason": outcome["skipped"],
                                        "variant": case_id.rsplit(".", 1)[0]}
            continue
        with tempfile.TemporaryDirectory() as work:
            executors = {}
            for case in cases(suite):
                case_id = case.id()
                if args.class_name and type(case).__name__ != args.class_name:
                    continue
                variant = f"{type(case).__module__}.{type(case).__qualname__}"
                if isinstance(reference[case_id], dict):
                    results[case_id] = {"result": "reference-skip",
                                        "reason": reference[case_id]["skipped"], "variant": variant}
                    continue
                if isinstance(case, doctest.DocTestCase):
                    results[case_id] = {"result": "separate-doctest-gate", "variant": f"{variant}: {case_id}"}
                    continue
                method = case._testMethodName
                original = inspect.unwrap(getattr(type(case), method))
                relative = Path(inspect.getsourcefile(original)).relative_to(upstream)
                if relative.as_posix() not in manifest["files"]:
                    raise ValueError(f"{case_id} uses an unverified method in {relative}")
                path = HERE / "upstream" / relative
                owner = original.__qualname__.split(".")[-2]
                node, fixture = native_fixture(path, owner, method, case)
                operation, assertions = None, "imre"
                if case_id == f"test.test_collections.TestCounter.{method}" and method in COUNTER_OPERATIONS:
                    fixture = UNARY_ASSERTIONS if method == "test_unary" else assertion_fixture(method, "NativeCase")
                    operation = ("imre", COUNTER_OPERATIONS[method])
                elif case_id == f"test.test_collections.TestChainMap.{method}" and method in CHAINMAP_OPERATIONS:
                    fixture = chainmap_fixture(method)
                    operation = ("imre", CHAINMAP_OPERATIONS[method])
                elif case_id == f"test.test_deque.TestBasic.{method}" and method in DEQUE_METHODS:
                    fixture, operation = deque_fixture(method, sys.modules[name].BIG)
                elif case_id == f"test.test_deque.TestBasic.{method}" and method in DEQUE_OPERATIONS:
                    operation = ("imre", DEQUE_OPERATIONS[method])
                elif case_id == "test.test_deque.TestBasic.test_basics":
                    operation = ("imre", "Reinitialize, grow, iterate and pop a native deque; reverse the list of right pops.")
                elif case_id == "test.test_ordered_dict.CPythonOrderedDictTests.test_setitem":
                    from test.test_ordered_dict import c_coll

                    if type(case).OrderedDict is not c_coll.OrderedDict:
                        raise ValueError(f"unexpected constructor for {case_id}")
                    fixture = ORDERED_SETITEM
                    operation = ("imre", "Native OrderedDict constructs ordered pairs, replaces an existing key, "
                                 "inserts a new key and iterates items in insertion order")
                elif case_id in EMPTY:
                    if len(node.body) != 1 or not isinstance(node.body[0], ast.Pass):
                        raise ValueError(f"{case_id} is no longer an empty upstream test")
                    operation, assertions = ("none", "The unchanged method body is pass."), "none"
                elif case_id == "test.test_userstring.UserStringTest.test_replacement_on_buffer_boundary":
                    operation = ("none", "The inherited body calls str.replace on literal strings; it never uses UserString.")
                    assertions = "none"
                directory = Path(work) / case_id
                directory.mkdir()
                record = {"source": f"{relative}:{node.lineno}", "variant": variant}
                try:
                    executors[case_id] = native_method(tools, directory, path, owner, case_id, fixture,
                                                       operation=operation, assertions=assertions)
                except (ValueError, RuntimeError) as exc:
                    record.update(failure(exc, directory, node, fixture))
                results[case_id] = record
            if executors:
                evidence, failures, skips = run_in_fixtures(suite, executors)
                for case_id, executed in evidence.items():
                    results[case_id].update(result="ran-pending-operation-audit", execution=executed)
                for case_id, detail in failures:
                    results[case_id].update(result="failed", reason=detail.replace(work, "<work>"))
                for case_id, detail in skips:
                    results[case_id].update(result="fixture-skip", reason=detail)
                if any("result" not in results[key] for key in executors):
                    raise ValueError("missing native routine outcome")
    expected = {name for name in reference if not args.module or name.startswith(args.module + ".")
                or (args.module == "test.test_collections" and name.startswith("collections."))}
    if not args.class_name and expected != set(results):
        raise ValueError(f"routine IDs differ from the reference: {expected ^ set(results)}")
    report = {"cpython_commit": manifest["commit"], "compiler_revision": args.compiler_revision,
              "opt_level": args.opt_level, "reference_denominator": len(reference),
              "routines": results}
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
