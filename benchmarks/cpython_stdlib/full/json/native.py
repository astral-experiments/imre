"""Run complete upstream JSON methods with host fixtures and native module calls."""

import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from reference import BENCH, COMMIT, cases, routine, verify

sys.path.insert(0, str(BENCH))
from native_json import METHODS as ERROR_METHODS, sha256
from native_json_values import METHODS as VALUE_METHODS, Native, unpack_native_repr


def counts():
    methods = {
        f"test.test_json.test_{group.lower()}.Test{group}.test_{method}": count
        for group, values in VALUE_METHODS.items() for method, count in values.items()
    }
    methods.update({f"test.test_json.test_decode.TestDecode.{method}": count
                    for method, count in ERROR_METHODS.items()})
    methods.update({
        "test.test_json.test_decode.TestDecode.test_decoder_optimizations": 1,
        "test.test_json.test_fail.TestFail.test_failures": 34,
    })
    return methods


def binary_stream_controls(native):
    # Read the original binary stream with json.load; the host transports bytes.
    import codecs
    source = '"µ☀𝄠"'
    for encoding, bom in (("utf-8", codecs.BOM_UTF8),
                          ("utf-16-le", codecs.BOM_UTF16_LE), ("utf-16-be", codecs.BOM_UTF16_BE),
                          ("utf-32-le", codecs.BOM_UTF32_LE), ("utf-32-be", codecs.BOM_UTF32_BE)):
        data = source.encode(encoding)
        for prefix in (bom, b""):
            value = unpack_native_repr(native.invoke("load", "binary", prefix + data))
            if value != source[1:-1]:
                raise AssertionError((encoding, prefix, value))
        # Only the first BOM is an encoding marker. The other is part of the
        # decoded document and receives the ordinary invalid-token diagnostic.
        try:
            native.invoke("load", "binary", bom + bom + "1".encode(encoding))
        except ValueError as exc:
            if native.calls[-1]["status"] != "JSONDecodeError" or str(exc) != "Expecting value: line 1 column 1 (char 0)":
                raise AssertionError((encoding, str(exc))) from exc
        else:
            raise AssertionError("native reader accepted a second BOM")
    # Check the codec, byte range and reason on malformed input. UTF-8-sig
    # reports offsets after the BOM, while UTF-16 and UTF-32 include it.
    for data in (b'\xef\xbb\xbf\x80', b'\xff\xfe\x01', b'\xfe\xff\x01',
                 b'\xff\xfe\x00\x00\x01', b'\x00\x00\xfe\xff\x00\x11\x00\x00',
                 b'\xff\xfe\x00\x00\x00\x00\x11\x00'):
        try:
            json.loads(data)
        except UnicodeDecodeError as e:
            expected = str(e)
        else:
            raise AssertionError("expected invalid binary control")
        try:
            native.invoke("load", "binary", data)
        except UnicodeDecodeError as e:
            if str(e) != expected:
                raise AssertionError((data, str(e), expected)) from e
        else:
            raise AssertionError("native reader accepted invalid binary control")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + arg, required=True, type=Path)
    args = parser.parse_args()
    verify()
    expected = counts()
    driver = BENCH / "json_values_driver.py"
    report = {"cpython_commit": COMMIT, "execution": "host unittest, native JSON calls",
              "sha256": {k: sha256(v) for k, v in (("imre", args.imre), ("runtime", args.runtime),
                         ("clang", args.clang), ("driver", driver))}, "levels": {}}
    ok = True
    with tempfile.TemporaryDirectory() as temp:
        directory = Path(temp)
        for level in ("0", "3"):
            program, ir = directory / "driver", directory / "driver.ll"
            subprocess.run([args.imre.resolve(), "build", driver, "-o", program, "--no-cache",
                            "--opt-level", level, "--runtime", args.runtime.resolve(),
                            "--clang", args.clang.resolve(), "--emit-llvm", ir], cwd=directory,
                           check=True, timeout=180)
            llvm = ir.read_text()
            for signature in ("ptr @imre_json_loads(", "ptr @imre_json_loads_bytes(",
                              "ptr @imre_json_dumps(", "i32 @imre_json_dump("):
                if "call " + signature not in llvm:
                    raise RuntimeError(f"missing native call: {signature}")
            results = {}
            for case in cases():
                source = routine(case)
                if source not in expected:
                    continue
                native = Native(program, directory, case.JSONDecodeError)
                case.json, case.loads, case.dumps = native, native.loads, native.dumps
                result = unittest.TestResult()
                case.run(result)
                calls = len(native.calls)
                passed = (result.testsRun == 1 and result.wasSuccessful() and not result.skipped
                          and calls == expected[source])
                results[case.id()] = {"source": source, "native_calls": calls,
                                      "status": "pass" if passed else "fail"}
                if not passed:
                    ok = False
                    results[case.id()]["diagnostics"] = [
                        msg for _, msg in [*result.errors, *result.failures, *result.skipped]
                    ] or [f"expected {expected[source]} native calls, got {calls}"]
                    print(case.id(), results[case.id()]["diagnostics"], file=sys.stderr)
            if {r["source"] for r in results.values()} != expected.keys():
                raise RuntimeError("some selected upstream routines were not discovered")
            binary_stream_controls(Native(program, directory, ValueError))
            report["levels"][level] = {"binary_stream_controls": 21, "tests": results}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(not ok)


if __name__ == "__main__":
    sys.exit(main())
