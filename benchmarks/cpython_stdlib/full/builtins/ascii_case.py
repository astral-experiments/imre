"""Replay stock CPython test_ascii assertions against native results for supported inputs."""

import argparse
import builtins
from collections import Counter
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent


def classify(value):
    if isinstance(value, str) and any(0xD800 <= ord(c) <= 0xDFFF for c in value):
        return "surrogate"
    if isinstance(value, list) and any(v is value for v in value):
        return "cycle"
    if isinstance(value, dict) and any(v is value for v in value.values()):
        return "cycle"
    assert type(value) in (str, int, tuple, list, dict)
    return "native"


def fixture(module, operation):
    result = unittest.TestResult()
    with patch.dict(module.__dict__, {"ascii": operation}):
        module.BuiltinTest("test_ascii").run(result)
    assert result.testsRun == 1 and not result.errors and not result.skipped, result.errors
    return result


def main():
    from test import test_builtin

    assert sys.version_info[:3] == (3, 14, 5), "use pinned test-enabled CPython"
    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    for path in (HERE / "upstream/Lib/test/test_builtin.py", Path(test_builtin.__file__)):
        data = path.read_bytes()
        assert hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest() == manifest["files"]["Lib/test/test_builtin.py"]
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("compiler", "runtime", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    cases = []

    def record(value):
        cases.append((classify(value), builtins.ascii(value)))
        return cases[-1][1]

    assert fixture(test_builtin, record).wasSuccessful()
    counts = Counter(kind for kind, _ in cases)
    assert counts == {"native": 14, "cycle": 2, "surrogate": 3}, counts
    literals = [literal for kind, literal in cases if kind == "native"]
    rows = {}
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        source = work / "builtin_ascii.py"
        source.write_text("".join(f"print(ascii({literal}))\n" for literal in literals))
        for level in ("0", "3"):
            binary = work / f"case-{level}"
            cmd = [args.compiler.resolve(), "build", source, "--module-root", work,
                   "--runtime", args.runtime.resolve(), "--opt-level", level, "--quiet", "-o", binary]
            subprocess.run(cmd + (["--sanitize"] if args.sanitize else []), check=True)
            output = subprocess.run([binary], capture_output=True, check=True)
            assert not output.stderr, output.stderr
            replies = output.stdout.decode("ascii").splitlines()
            assert len(replies) == counts["native"]

            def replay(wrong):
                inputs, results = iter(cases), iter(replies)

                def operation(value):
                    kind, original = next(inputs)
                    assert (classify(value), builtins.ascii(value)) == (kind, original)
                    if kind != "native":
                        return original
                    reply = next(results)
                    return "injected wrong native reply" if wrong and value == "𝄡" else reply

                result = fixture(test_builtin, operation)
                assert len(result.failures) == int(wrong), result.failures
                if not wrong:
                    assert next(inputs, None) is None and next(results, None) is None

            replay(False)
            replay(True)
            rows[level] = {"native_calls": counts["native"], "cpython_cycle_calls": counts["cycle"],
                           "cpython_surrogate_calls": counts["surrogate"], "wrong_reply_failed_assertion": True}
    report = {"original_id": "test.test_builtin.BuiltinTest.test_ascii", "complete_native_methods": 0,
              "fixture_body_and_assertions": "CPython", "native_operation": "builtin ascii", "opt_levels": rows}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
