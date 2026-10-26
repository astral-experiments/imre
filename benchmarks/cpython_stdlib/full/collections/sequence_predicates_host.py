"""Run the unchanged MutableSequence test with native predicates and host ABCMeta."""

import argparse
import array
import builtins
from collections import Counter, deque
from collections.abc import MutableSequence
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

CASE = "test.test_collections.TestCollectionABCs.test_MutableSequence"
SUBJECTS = (tuple, str, bytes, list, bytearray, deque, array.array)
PROGRAM = b"""import sys
from array import array
from collections import deque
from collections.abc import MutableSequence

index = int(sys.argv[2])
if sys.argv[1] == "issubclass":
    subjects = [tuple, str, bytes, list, bytearray, deque, array]
    answer = issubclass(subjects[index], MutableSequence)
elif index == 0:
    answer = isinstance((), MutableSequence)
elif index == 1:
    answer = isinstance("", MutableSequence)
elif index == 2:
    answer = isinstance(b"", MutableSequence)
elif index == 3:
    subject: list[int] = []
    answer = isinstance(subject, MutableSequence)
elif index == 5:
    queue: deque[int] = deque()
    answer = isinstance(queue, MutableSequence)
else:
    raise ValueError("unsupported instance transport")
print(answer)
"""


def exercise(child, method_sha256, level, control=None):
    from test import test_collections
    native, host, results = Counter(), Counter(), []

    def dispatch(op, subject):
        result = subprocess.run([child, op, str(SUBJECTS.index(subject))], capture_output=True, timeout=30, check=True)
        if result.stderr or result.stdout not in (b"True\n", b"False\n"):
            raise AssertionError((result.stdout, result.stderr))
        answer = result.stdout == b"True\n"
        native[op] += 1
        results.append([op, subject.__module__ + "." + subject.__qualname__, answer])
        return not answer if (op, subject) == control else answer

    def check_subclass(subject, cls):
        return dispatch("issubclass", subject) if cls is MutableSequence else builtins.issubclass(subject, cls)

    def check_instance(obj, cls):
        if cls is not MutableSequence:
            return builtins.isinstance(obj, cls)
        if type(obj) is bytearray:
            host["bytearray isinstance"] += 1
            return builtins.isinstance(obj, cls)
        if len(obj) != 0:
            raise AssertionError("this transport accepts only the original empty operands")
        return dispatch("isinstance", type(obj))

    def make_class(name, bases, methods):
        if bases != (MutableSequence,):
            raise AssertionError(bases)
        host["type(name, bases, methods)"] += 1
        return builtins.type(name, bases, methods)

    def execute(case, original):
        with ExitStack() as stack:
            for target, name, function in ((unittest.case, "issubclass", check_subclass),
                                           (unittest.case, "isinstance", check_instance),
                                           (test_collections, "type", make_class)):
                stack.enter_context(patch.object(target, name, function, create=True))
            original()
        if native != {"issubclass": 8, "isinstance": 5} or host != {"bytearray isinstance": 1, "type(name, bases, methods)": 6}:
            raise AssertionError((native, host))
        return {"observation": {
            "roles": roles(body="cpython", assertions="cpython", operation="mixed", fixture="cpython"),
            "evidence": {"method_sha256": method_sha256, "opt_level": level,
                         "operation": "Imre evaluated eight subclass and five instance predicates. CPython checked the bytearray instance and constructed the ABC subclasses with their actual method dictionaries.",
                         "fixture": "Original CPython fixtures, method, helper and unittest assertions.",
                         "native_calls": native, "host_calls": host, "native_results": results},
        }}
    return run_in_fixtures(load_suite("test.test_collections"), {CASE: execute})


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        p.add_argument("--" + name, type=Path, required=True)
    p.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    p.add_argument("--compiler-revision", required=True)
    args = p.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        p.error("use a test-enabled CPython 3.14.5")
    from test import test_collections

    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    verify_files(HERE / "upstream", manifest["files"])
    verify_files(Path(test_collections.__file__).resolve().parents[2], manifest["files"])
    method = method_bytes(HERE / "upstream/Lib/test/test_collections.py", "TestCollectionABCs", "test_MutableSequence")
    method_sha = hashlib.sha256(method).hexdigest()
    with tempfile.TemporaryDirectory() as work:
        child = Toolchain(args.imre, args.runtime, args.clang, args.opt_level).build(PROGRAM, Path(work))
        tests, failures, skips = exercise(child, method_sha, args.opt_level)
        if failures or skips or CASE not in tests:
            raise AssertionError((failures, skips))
        controls = {}
        for op, subject in (("issubclass", array.array), ("isinstance", list)):
            _, failed, skipped = exercise(child, method_sha, args.opt_level, (op, subject))
            if skipped or len(failed) != 1 or failed[0][0] != CASE or "AssertionError" not in failed[0][1]:
                raise AssertionError((op, subject, failed, skipped))
            controls[f"{op} {subject.__name__}"] = failed[0][1].strip().splitlines()[-1]
    args.result.write_text(json.dumps({"cpython_commit": manifest["commit"], "compiler_revision": args.compiler_revision,
                                       "opt_level": args.opt_level, "tests": tests, "failures": [], "skips": [],
                                       "controls": controls}, indent=2) + "\n")


if __name__ == "__main__":
    main()
