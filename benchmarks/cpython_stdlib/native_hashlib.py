"""Run CPython's four SHA-256 vectors with the public one-shot hexdigest on Relar."""
import argparse
from collections import Counter
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from upstream import verify_upstream

DRIVER = """import hashlib
import sys

with open(sys.argv[1], "rb") as stream:
    print(hashlib.sha256(stream.read()).hexdigest())
"""


def run_case(suite, name, program, payload):
    case = suite.HashLibTestCase(name)
    original = suite.hashlib.sha256
    constructors = case.constructors_to_test["sha256"]
    constructors.remove(original)
    host_constructors = {f"{ctor.__module__}.{ctor.__name__}": mock.Mock(wraps=ctor) for ctor in constructors}
    constructors.clear()
    constructors.update(host_constructors.values())
    host_calls = Counter()
    lengths = []

    class OneShot:
        def __init__(self, data):
            self.data = data
            self.reference = original(data)

        def hexdigest(self):
            payload.write_bytes(self.data)
            result = subprocess.run([program, payload], check=True, capture_output=True, timeout=30)
            if result.stderr:
                raise AssertionError(result.stderr)
            lengths.append(len(self.data))
            return result.stdout.decode("ascii").rstrip("\n")

        def digest(self):
            host_calls["direct digest"] += 1
            return self.reference.digest()

        @property
        def digest_size(self):
            host_calls["direct digest_size"] += 1
            return self.reference.digest_size

    def direct(*args):
        if args:  # The vector calls the public constructor with its input bytes.
            return OneShot(*args)
        # The file_digest helper requests an empty, mutable host constructor.
        host_calls["empty constructor for file_digest"] += 1
        return original()

    constructors.add(direct)
    with mock.patch.object(suite.hashlib, "file_digest", wraps=suite.hashlib.file_digest) as files:
        result = unittest.TestResult()
        case.run(result)
    host_calls["file_digest"] = files.call_count
    passed = result.wasSuccessful() and result.testsRun == 1 and not result.skipped and len(lengths) == 1
    return case.id(), {
        "status": "mixed" if passed else "fail",
        "native_one_shot_input_bytes": lengths,
        "host_operations": dict(host_calls),
        "host_reference_constructors": {name: fn.call_count for name, fn in sorted(host_constructors.items())},
        "diagnostics": [reason for _, reason in result.failures + result.errors + result.skipped],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("relar", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("the original hashlib fixtures require CPython 3.14.5")
    commit = verify_upstream()
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    vendored = HERE / "upstream/Lib/test"
    test.__path__.insert(0, str(vendored))
    from test import test_hashlib
    if Path(test_hashlib.__file__).resolve() != vendored / "test_hashlib.py":
        raise RuntimeError("the selected methods must come from the verified vendored file")

    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        source, program = work / "hash.py", work / "hash"
        source.write_text(DRIVER)
        subprocess.run([args.relar.resolve(), "build", source, "-o", program, "--no-cache",
                        "--opt-level", "3", "--clang", args.clang.resolve(), "--runtime", args.runtime.resolve()],
                       check=True, timeout=120)
        cases = dict(run_case(test_hashlib, f"test_case_sha256_{n}", program, work / "data") for n in range(4))
    report = {"cpython_commit": commit, "whole_native_methods": 0, "tests": cases}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    for case, row in cases.items():
        print(row["status"], case, row["native_one_shot_input_bytes"])
    return int(any(row["status"] == "fail" for row in cases.values()))


if __name__ == "__main__":
    sys.exit(main())
