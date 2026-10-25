"""Run the original Counter helper test with a native exact-dict operation."""

import argparse
import ast
import collections
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import roles
from suite_inventory import load_suite, verify_files
from suite_native import Toolchain, method_bytes, run_in_fixtures

CASE = "test.test_collections.TestCounter.test_helper_function"
PROGRAM = b"""from _collections import _count_elements
from sys import argv

counts: dict[str, int] = {}
pairs = int(argv[1])
i = 2
for j in range(pairs):
    counts[argv[i]] = int(argv[i + 1])
    i += 2
keys: list[str] = []
while i < len(argv):
    keys.append(argv[i])
    i += 1
_count_elements(counts, keys)
for key, value in counts.items():
    print(repr(key))
    print(value)
"""


def exercise(child, digest, control=False):
    from test import test_collections

    native, host = {}, {}
    host_count = collections._count_elements

    def count(mapping, iterable):
        if type(mapping) is not dict:
            name = type(mapping).__name__
            host[name] = host.get(name, 0) + 1
            return host_count(mapping, iterable)
        if not all(type(k) is str and type(v) is int for k, v in mapping.items()):
            raise TypeError("only a str-to-int dictionary can cross this fixture boundary")
        if type(iterable) is not list or not all(type(k) is str for k in iterable):
            raise TypeError("expected the original list of strings")
        args = [child, str(len(mapping)), *(s for k, v in mapping.items() for s in (k, str(v))), *iterable]
        result = subprocess.run(args, capture_output=True, text=True, timeout=30, check=True)
        if result.stderr:
            raise AssertionError(result.stderr)
        words = iter(result.stdout.splitlines())
        pairs = [(ast.literal_eval(k), int(v)) for k, v in zip(words, words, strict=True)]
        if control:
            k, v = pairs[0]
            pairs[0] = (k, v + 1)
        mapping.clear()
        mapping.update(pairs)
        native["dict"] = native.get("dict", 0) + 1

    def execute(case, original):
        with patch.object(test_collections, "_count_elements", count), patch.object(collections, "_count_elements", count):
            original()
        if native != {"dict": 1} or host != {"OrderedDict": 1, "CounterSubclassWithGet": 1, "CounterSubclassWithSetItem": 1}:
            raise AssertionError((native, host))
        return {"observation": {
            "roles": roles(body="cpython", assertions="cpython", operation="mixed", fixture="cpython"),
            "evidence": {"method_sha256": digest, "native_calls": native, "host_calls": host,
                         "operation": "Imre counted the keys for the exact dict; its emitted pairs reconstruct the receiver. CPython executed the OrderedDict and both real Counter subclass operations.",
                         "fixture": "Original CPython module, class, instance, method, and unittest assertions."},
        }}
    return run_in_fixtures(load_suite("test.test_collections"), {CASE: execute})


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
    method = method_bytes(HERE / "upstream/Lib/test/test_collections.py", "TestCounter", "test_helper_function")
    digest = hashlib.sha256(method).hexdigest()
    with tempfile.TemporaryDirectory() as work:
        child = Toolchain(args.imre, args.runtime, args.clang, args.opt_level).build(PROGRAM, Path(work))
        tests, failures, skips = exercise(child, digest)
        if failures or skips or CASE not in tests:
            raise AssertionError((failures, skips))
        _, failed, skipped = exercise(child, digest, control=True)
        if skipped or len(failed) != 1 or failed[0][0] != CASE or "AssertionError" not in failed[0][1]:
            raise AssertionError((failed, skipped))
    assertion = next(line for line in failed[0][1].splitlines() if line.startswith("AssertionError:"))
    args.result.write_text(json.dumps({"cpython_commit": manifest["commit"], "compiler_revision": args.compiler_revision,
        "opt_level": args.opt_level, "tests": tests, "failures": [], "skips": [],
        "controls": {"wrong native count": assertion}}, indent=2) + "\n")


if __name__ == "__main__":
    main()
