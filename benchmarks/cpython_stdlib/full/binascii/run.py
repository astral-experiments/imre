"""Run the unchanged bytes-fixture methods and compare the public codecs with CPython."""
import argparse
import ast
import binascii
import itertools
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import build_report, roles, write_reports
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, method_bytes

NATIVE = ("test_base64valid", "test_b2a_base64_newline")
HOSTED = ("test_base64invalid", "test_base64_strict_mode", "test_base64_excess_data", "test_base64errors")
SOURCE = HERE / "upstream/Lib/test/test_binascii.py"


class Native:
    def __init__(self, binary, root):
        self.binary, self.root, self.calls = binary, root, 0

    def batch(self, operation, inputs, flag):
        paths = []
        for i, data in enumerate(inputs):
            path = self.root / f"input-{i}"
            path.write_bytes(data.encode() if isinstance(data, str) else data)
            paths.append(path)
        result = subprocess.run([self.binary, operation, str(int(flag)), *paths],
                                check=True, capture_output=True, text=True, timeout=60)
        lines = result.stdout.splitlines()
        if result.stderr or len(lines) != 2 * len(inputs):
            raise AssertionError((result.stderr, lines))
        self.calls += len(inputs)
        return [(kind, ast.literal_eval(value) if kind == "ok" else value)
                for kind, value in zip(lines[::2], lines[1::2], strict=True)]

    def call(self, operation, data, flag):
        (kind, value), = self.batch(operation, [data], flag)
        if kind == "ok":
            return value
        raise {"binascii.Error": binascii.Error, "ValueError": ValueError}[kind](value)

    def b2a_base64(self, data, /, *, newline=True):
        if type(data) is not bytes:
            raise AssertionError(f"the native worker requires bytes, got {type(data)}")
        return self.call("encode", data, newline)

    def a2b_base64(self, data, /, *, strict_mode=False):
        if type(data) not in (bytes, str):
            raise AssertionError(f"the native worker requires bytes or str, got {type(data)}")
        return self.call("text" if isinstance(data, str) else "decode", data, strict_mode)


def compare_vectors(native):
    encoded = [bytes(range(256))[:i] for i in range(257)]
    decode = [bytes(seq) for n in range(6) for seq in itertools.product(b"A/!=\n", repeat=n)]
    decode += [b"AA" + bytes([i]) + b"A==" for i in range(256)]
    text = ["", "YQ==", "ab==c=", "A", "!!!!", "é", "Zg==é", "π", "🐍"]
    count = 0
    for flag in (False, True):
        for op, inputs in (("encode", encoded), ("decode", decode), ("text", text)):
            for start in range(0, len(inputs), 256):
                batch = inputs[start:start + 256]
                actual = native.batch(op, batch, flag)
                for data, row in zip(batch, actual, strict=True):
                    try:
                        result = (binascii.b2a_base64(data, newline=flag) if op == "encode"
                                  else binascii.a2b_base64(data, strict_mode=flag))
                    except binascii.Error as exc:
                        expected = ("binascii.Error", str(exc))
                    except ValueError as exc:
                        expected = ("ValueError", str(exc))
                    else:
                        expected = ("ok", result)
                    if expected != row:
                        raise AssertionError((op, data, flag, expected, row))
                    count += 1
    return count


def original_program(rawdata):
    fixture = f'''import binascii

class BinASCIITest:
    rawdata = {rawdata!r}

    def type2test(self, data: bytes) -> bytes:
        return bytes(data)

    def assertEqual(self, value: bytes, expected: bytes) -> None:
        if value != expected:
            raise AssertionError(f"{{value!r}} != {{expected!r}}")
'''.encode()
    bodies = b"\n".join(method_bytes(SOURCE, "BinASCIITest", name) for name in NATIVE)
    run = "\ncase = BinASCIITest()\n" + "".join(f"case.{name}()\n" for name in NATIVE)
    return fixture + b"\n" + bodies + run.encode()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--imre", required=True, type=Path)
    p.add_argument("--runtime", required=True, type=Path)
    p.add_argument("--clang", required=True, type=Path)
    p.add_argument("--reachable-only", action="store_true")
    p.add_argument("--result", required=True, type=Path)
    args = p.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        p.error("run on the pinned CPython 3.14.5")
    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    from test import test_binascii as suite
    ref = json.loads((HERE / "reference-linux.json").read_bytes())
    discovered = [(case.id(), type(case).__name__) for case in cases(load_suite(manifest["module"]))]
    locations = {node.name: min([node.lineno, *(d.lineno for d in node.decorator_list)])
                 for node in ast.walk(ast.parse(SOURCE.read_bytes())) if isinstance(node, ast.FunctionDef)}
    reports = []
    for level in (0, 3):
        lang = Toolchain(args.imre, args.runtime, args.clang, level, args.reachable_only)
        observed = {}
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            for sub in ("original", "negative", "worker"):
                (root / sub).mkdir()
            original = original_program(suite.BinASCIITest.rawdata)
            result = subprocess.run([lang.build(original, root / "original")], check=True, capture_output=True, timeout=60)
            if result.stdout or result.stderr:
                raise AssertionError((result.stdout, result.stderr))
            fail = original + b'\ncase.assertEqual(b"first", b"second")\n'
            failed = subprocess.run([lang.build(fail, root / "negative")], capture_output=True, timeout=60)
            if not failed.returncode or b"AssertionError: b'first' != b'second'" not in failed.stderr:
                raise AssertionError(failed)
            for name in NATIVE:
                observed[suite.BinASCIITest(name).id(), 1] = {
                    "outcome": "pass", "roles": roles(body="imre", assertions="imre", operation="imre", fixture="mixed"),
                    "evidence": {"operation": "Complete original method, byte equality, and binascii calls ran on Imre.",
                                 "fixture": "CPython supplied rawdata; the generated bytes fixture supplies type2test and the checked byte assertion."}}
            native = Native(lang.build((HERE / "worker.py").read_bytes(), root / "worker"), root)
            proxy = SimpleNamespace(Error=binascii.Error, b2a_base64=native.b2a_base64, a2b_base64=native.a2b_base64)
            with patch.object(suite, "binascii", proxy):
                for name in HOSTED:
                    case, before = suite.BinASCIITest(name), native.calls
                    result = case.run(unittest.TestResult())
                    if result.failures or result.errors or result.skipped or native.calls <= before:
                        raise AssertionError((case.id(), result.failures, result.errors, result.skipped))
                    observed[case.id(), 1] = {
                        "outcome": "pass", "roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="cpython"),
                        "evidence": {"operation": f"{native.calls-before} native codec calls, with exact values or exception class and message.",
                                     "fixture": "Original CPython bytes fixture, assertions, and input construction."}}
            vectors = compare_vectors(native)
            print(f"O{level}: {len(NATIVE)} compiled bodies, {len(HOSTED)} CPython-driven methods, {vectors} differential calls")
        skips = {}
        for key, variant in discovered:
            outcome = ref["reference"][key]
            if isinstance(outcome, dict) and "skipped" in outcome:
                skips[key, 1, level] = {
                    "kind": "upstream-skip", "reason": outcome["skipped"], "condition": outcome["skipped"],
                    "source": f"Lib/test/test_binascii.py:{locations[key.rsplit('.', 1)[1]]}",
                    "platform": sys.platform, "configuration": "CPython default resources without Hypothesis",
                    "fixture_variant": variant,
                }
        reports.append(build_report(ref, level, discovered, observed, exceptions=skips))
    write_reports(args.result, reports, ref)


if __name__ == "__main__":
    main()
