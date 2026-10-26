"""Exercise the original Mapping and MutableMapping methods with native dict predicates."""

import argparse
import builtins
from collections import Counter
from collections.abc import Mapping, MutableMapping
from contextlib import ExitStack
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest.case
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import roles
from suite_inventory import load_suite, verify_files
from suite_native import Toolchain, method_bytes, run_in_fixtures

ABCS = (Mapping, MutableMapping)
SLOTS = ("__iter__", "__len__", "__getitem__", "__setitem__", "__delitem__")
PROGRAM = b"""import sys
from collections.abc import Mapping, MutableMapping

index = int(sys.argv[2])
if sys.argv[1] == "issubclass":
    parents = [Mapping, MutableMapping]
    subjects = [dict]
    answer = issubclass(subjects[0], parents[index])
else:
    values: dict[str, int] = {}
    if index == 0:
        answer = isinstance(values, Mapping)
    else:
        answer = isinstance(values, MutableMapping)
print(answer)
"""


def case_id(cls):
    return "test.test_collections.TestCollectionABCs.test_" + cls.__name__


def exercise(child, abc, digest, level, wrong=None):
    from test import test_collections

    native, host = Counter(), Counter()
    stubs, reflected = [], []
    index = ABCS.index(abc)
    slots = SLOTS if abc is MutableMapping else SLOTS[:3]

    def dispatch(operation):
        result = subprocess.run([child, operation, str(index)], capture_output=True, check=True, timeout=30)
        if result.stderr or result.stdout not in (b"True\n", b"False\n"):
            raise AssertionError((result.stdout, result.stderr))
        native[operation] += 1
        answer = result.stdout == b"True\n"
        return not answer if operation == wrong else answer

    def check_instance(subject, parent):
        if parent is not abc:
            return builtins.isinstance(subject, parent)
        if type(subject) is not dict or subject:
            raise AssertionError("expected the original empty dict")
        return dispatch("isinstance")

    def check_subclass(subject, parent):
        if parent is not abc:
            return builtins.issubclass(subject, parent)
        if subject is not dict:
            raise AssertionError("expected the original dict class")
        return dispatch("issubclass")

    def original_type(*args):
        if len(args) == 3:
            name, bases, methods = args
            if name != "C" or bases != (abc,):
                raise AssertionError((name, bases))
            stubs.append(list(methods))
            host["type(name, bases, methods)"] += 1
        return builtins.type(*args)

    def original_reversed(instance):
        host["reversed(MyMapping())"] += 1
        return builtins.reversed(instance)

    def execute(case, original):
        expected_stubs = [list(slots)] + [[s for s in slots if s != missing] for missing in slots]
        with ExitStack() as stack:
            for target, name, function in ((unittest.case, "issubclass", check_subclass),
                                           (unittest.case, "isinstance", check_instance),
                                           (test_collections, "type", original_type)):
                stack.enter_context(patch.object(target, name, function, create=True))
            if abc is Mapping:
                stack.enter_context(patch.object(test_collections, "reversed", original_reversed, create=True))
                original_assert = case.assertTrue

                def observe_reflection(ok, msg=None):
                    if msg and msg.startswith("Right side not called for "):
                        reflected.append(msg.rsplit(".", 1)[-1])
                        host["reflected comparison"] += 1
                    return original_assert(ok, msg)
                stack.enter_context(patch.object(case, "assertTrue", observe_reflection))
            original()
        expected_host = {"type(name, bases, methods)": len(expected_stubs)}
        if abc is Mapping:
            expected_host.update({"reflected comparison": 5, "reversed(MyMapping())": 1})
        if (native != {"isinstance": 1, "issubclass": 1} or host != expected_host or stubs != expected_stubs
                or (abc is Mapping and reflected != ["__lt__", "__gt__", "__le__", "__ge__", "__ne__"])):
            raise AssertionError((native, host, stubs, reflected))
        return {"observation": {
            "roles": roles(body="cpython", assertions="cpython", operation="mixed", fixture="cpython"),
            "evidence": {"method_sha256": digest, "opt_level": level, "native_calls": native, "host_calls": host,
                         "operation": "Imre checked the original dict instance and dict class against the ABC. CPython constructed and instantiated the abstract/concrete classes, and performed the remaining operations.",
                         "fixture": "Original method, helper, assertions and unittest fixtures run by CPython.",
                         "host_method_dictionaries": stubs, "host_reflected_operators": reflected},
        }}
    return run_in_fixtures(load_suite("test.test_collections"), {case_id(abc): execute})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    parser.add_argument("--compiler-revision", required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use a test-enabled CPython 3.14.5")
    from test import test_collections

    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    verify_files(HERE / "upstream", manifest["files"])
    verify_files(Path(test_collections.__file__).resolve().parents[2], manifest["files"])
    tests, controls = {}, {}
    with tempfile.TemporaryDirectory() as work:
        child = Toolchain(args.imre, args.runtime, args.clang, args.opt_level).build(PROGRAM, Path(work))
        for abc in ABCS:
            case = case_id(abc)
            original = method_bytes(HERE / "upstream/Lib/test/test_collections.py", "TestCollectionABCs",
                                    "test_" + abc.__name__)
            digest = hashlib.sha256(original).hexdigest()
            observed, failed, skipped = exercise(child, abc, digest, args.opt_level)
            if failed or skipped or set(observed) != {case}:
                raise AssertionError((case, failed, skipped))
            tests.update(observed)
            controls[case] = {}
            for op, assertion in (("isinstance", "self.assertIsInstance"), ("issubclass", "self.assertIsSubclass")):
                _, failed, skipped = exercise(child, abc, digest, args.opt_level, op)
                if (skipped or len(failed) != 1 or failed[0][0] != case
                        or assertion + "(" not in failed[0][1] or "AssertionError" not in failed[0][1]):
                    raise AssertionError((case, op, failed, skipped))
                controls[case][op] = failed[0][1].strip().splitlines()[-1]
    args.result.write_text(json.dumps({"cpython_commit": manifest["commit"], "compiler_revision": args.compiler_revision,
                                       "opt_level": args.opt_level, "tests": tests, "controls": controls,
                                       "failures": [], "skips": []}, indent=2) + "\n")


if __name__ == "__main__":
    main()
