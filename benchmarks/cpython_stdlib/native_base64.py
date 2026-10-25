"""Compare URL-safe encoding and strict ASCII decoding to the pinned CPython stdlib."""
import argparse
import ast
import base64
from collections import Counter
import importlib.util
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

DRIVER = '''import sys
from base64 import urlsafe_b64encode
from pathlib import Path

data = Path(sys.argv[2]).read_bytes()
if sys.argv[1] == "encode":
    print(repr(urlsafe_b64encode(s=data)))
else:
    try:
        print("ok " + repr(data.decode("ascii")))
    except UnicodeDecodeError as error:
        print("error " + str(error))
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    parser.add_argument("--opt-level", choices=("0", "3"), default="3")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("the oracle requires CPython 3.14.5")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    commit = verify_upstream()
    spec = importlib.util.spec_from_file_location("pinned_test_base64", HERE / "upstream/Lib/test/test_base64.py")
    suite = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(suite)
    reference = base64.urlsafe_b64encode
    calls = Counter()
    with tempfile.TemporaryDirectory() as directory:
        work = Path(directory)
        driver, executable, payload = (work / name for name in ("probe.py", "probe", "input"))
        driver.write_text(DRIVER)
        subprocess.run([args.imre.resolve(), "build", driver, "--opt-level", args.opt_level, "--no-cache",
                        "--runtime", args.runtime.resolve(), "--clang", args.clang.resolve(), "-o", executable],
                       check=True, timeout=120)
        def invoke(mode, data):
            payload.write_bytes(data)
            run = subprocess.run([executable, mode, payload], text=True, capture_output=True, timeout=30, check=True)
            if run.stderr:
                raise AssertionError(run.stderr)
            return run.stdout.removesuffix("\n")

        def encode(data):
            if type(data) is bytes:
                calls["native bytes"] += 1
                return ast.literal_eval(invoke("encode", data))
            calls["host " + type(data).__name__] += 1
            return reference(data)

        case = suite.BaseXYTestCase("test_b64encode")
        outcome = unittest.TestResult()
        with mock.patch.object(suite.base64, "urlsafe_b64encode", encode):
            case.run(outcome)
        if not outcome.wasSuccessful() or outcome.skipped or calls["native bytes"] != 1:
            raise AssertionError((outcome.errors, outcome.failures, outcome.skipped, calls))
        lengths = [0, 1, 2, 3, 4, 5, 31, 32, 33, 255, 256, 257, 4096]
        for n in lengths:
            data = (bytes(range(256)) * (n // 256 + 1))[:n]
            assert ast.literal_eval(invoke("encode", data)) == reference(data)
        samples = [b"", bytes(range(128)), b"A\xfe\xff", b"\xff", b"abc\xc3\xa9"]
        for data in samples:
            try:
                result = "ok " + repr(data.decode("ascii"))
            except UnicodeDecodeError as error:
                result = "error " + str(error)
            assert invoke("ascii", data) == result
    report = {"upstream_commit": commit, "opt_level": args.opt_level, "method": case.id(),
              "urlsafe_dispatch": calls, "other_base64_functions": "host CPython",
              "additional_native_input_lengths": lengths, "native_ascii_cases": len(samples)}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    print("pass", case.id(), dict(calls), len(lengths), "encoding controls,", len(samples), "ASCII controls")


if __name__ == "__main__":
    main()
