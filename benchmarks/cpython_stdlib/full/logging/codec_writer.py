"""Compare the codec writer, its backing store and all CP1251 byte mappings."""
import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import verify_files
from suite_native import Toolchain, method_bytes

CP1251 = "Lib/encodings/cp1251.py"
TEST = "Lib/test/test_logging.py"
STREAM_FIXTURE = HERE.parents[3] / "tests/fixtures/files/codec_writer.py"
FILES = {
    CP1251: "22bc66002d3e8a72d2dd42c79055b61d6b8e082b",
    "Lib/codecs.py": "e4a8010aba90a503aa25ae4a81eef68790150be3",
}


def table_program(root):
    tree = ast.parse((root / CP1251).read_bytes())
    table, = [ast.literal_eval(node.value) for node in tree.body
              if isinstance(node, ast.Assign) and ast.unparse(node.targets[0]) == "decoding_table"]
    assert len(table) == 256 and [i for i, c in enumerate(table) if c == "\ufffe"] == [0x98]
    valid = bytes(i for i, char in enumerate(table) if char != "\ufffe")
    text = "".join(table[i] for i in valid)
    return f"""import codecs
entry = codecs.lookup('windows-1251')
encoded, count = entry.encode({ascii(text)})
print(encoded == {valid!r})
print(count)
decoded, count = entry.decode({valid!r})
print(decoded == {ascii(text)})
print(count)
for policy in ['strict', 'replace', 'ignore', 'backslashreplace']:
    try:
        value, count = entry.decode(b'a\\x98z', policy)
        print(value)
        print(count)
    except UnicodeDecodeError as exc:
        print(str(exc))
""".encode()


def observe(command, directory):
    proc = subprocess.run(command, cwd=directory, capture_output=True, timeout=90)
    if proc.returncode or proc.stderr:
        raise AssertionError((proc.returncode, proc.stdout, proc.stderr))
    return proc.stdout


def reference_bytes(root, expected):
    """Run the original fixture and observe both operands of its real assertion."""
    from test import test_logging
    from unittest.mock import patch

    original = Path(test_logging.__file__)
    if original.read_bytes() != (root / TEST).read_bytes():
        raise ValueError(f"{original} differs from the verified CPython test")
    case = test_logging.EncodingTest("test_encoding_cyrillic_unicode")
    assertion = case.assertEqual
    observed = []

    def capture(left, right, *args, **kwargs):
        assertion(left, right, *args, **kwargs)
        if isinstance(left, bytes):
            observed.append((left, right))

    result = unittest.TestResult()
    with patch.object(case, "assertEqual", capture):
        unittest.TestSuite([case]).run(result)
    if not result.wasSuccessful() or result.skipped or observed != [(expected, expected)]:
        raise AssertionError((result.errors, result.failures, result.skipped, observed))
    return {"test_id": case.id(), "bytes_hex": observed[0][0].hex(), "length": len(expected)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "result", "cpython-source"):
        parser.add_argument("--" + arg, required=True, type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use the pinned CPython 3.14.5")
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(args.cpython_source, manifest["files"] | FILES)
    case = method_bytes(args.cpython_source / TEST, "EncodingTest", "test_encoding_cyrillic_unicode")
    expected_bytes, = [node.value for node in ast.walk(ast.parse(b"class EncodingTest:\n" + case))
                       if isinstance(node, ast.Constant) and isinstance(node.value, bytes)]
    assert expected_bytes.hex() == "e4ee20f1e2e8e4e0ede8ff0a"
    report = {"cpython_commit": manifest["commit"], "verified_blobs": manifest["files"] | FILES,
              "upstream_expected_hex": expected_bytes.hex(), "valid_codepage_bytes": 255,
              "undefined_byte": "98", "logging_occurrences": 0,
              "cpython_reference_method": reference_bytes(args.cpython_source, expected_bytes),
              "levels": {}}
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        for label, source in (("stream", STREAM_FIXTURE.read_bytes()),
                              ("table", table_program(args.cpython_source))):
            script = work / "reference.py"
            script.write_bytes(source)
            expected = observe([sys.executable, "-I", "-B", script], work)
            if label == "stream":
                assert repr(expected_bytes).encode() in expected.splitlines()
            else:
                assert expected.splitlines()[:4] == [b"True", b"255", b"True", b"255"]
            for level in (0, 3):
                tools = Toolchain(args.imre, args.runtime, args.clang, level)
                actual = observe([tools.build(source, work)], work)
                if actual != expected:
                    raise AssertionError((label, level, expected, actual))
                report["levels"].setdefault(str(level), {})[label] = {
                    "stdout_sha256": hashlib.sha256(actual).hexdigest(),
                    "observations": len(actual.splitlines()),
                    "source_sha256": hashlib.sha256(source).hexdigest(),
                }
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    print("All CP1251 byte mappings and stream observations matched at O0 and O3.")


if __name__ == "__main__":
    main()
