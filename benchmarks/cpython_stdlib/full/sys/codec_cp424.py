"""Compare the native registry with the table used by CPython's test_ioencoding."""

import argparse
import ast
import encodings.cp424
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

SHARED = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(SHARED))
from suite_inventory import verify_files
from suite_native import Toolchain

TABLE = "Lib/encodings/cp424.py"
TABLE_BLOB = "6753daf122df44ab9a4fb28b4fc493e2a521ddde"
TEST = "Lib/test/test_sys.py"
FILES = {TEST: "0db0369500a7c7c965386bda5646046978fec3c7",
         "LICENSE": "20cf39097c68baa17cc566b64e76d34ebf034044"}


def mappings(root):
    # The hosted checkout contains Lib/test; the pinned interpreter ships the codec.
    source = Path(encodings.cp424.__file__).read_bytes()
    if hashlib.sha1(f"blob {len(source)}\0".encode() + source).hexdigest() != TABLE_BLOB:
        raise ValueError("CPython CP424 blob mismatch")
    if (root / TABLE).exists():
        verify_files(root, {TABLE: TABLE_BLOB})
    table, = [ast.literal_eval(n.value) for n in ast.parse(source).body if isinstance(n, ast.Assign)
              and any(isinstance(t, ast.Name) and t.id == "decoding_table" for t in n.targets)]
    defined = bytes(i for i, ch in enumerate(table) if ch != "\ufffe")
    missing = [bytes([i]) for i, ch in enumerate(table) if ch == "\ufffe"]
    assert len(table) == 256 and len(defined) == 218 and len(missing) == 38
    return defined, "".join(table[i] for i in defined), missing


def source_program(defined, text, missing):
    return f"""import codecs
entry = codecs.lookup("CP424")
for alias in ["cp424", "424", "CSIBM424", "EBCDIC-CP-HE", "IBM424"]:
    print(codecs.lookup(alias).name)
raw, count = entry.encode({ascii(text)})
print(raw == {defined!r})
print(count)
value, count = entry.decode({defined!r})
print(value == {ascii(text)})
print(count)
for raw in {missing!r}:
    try:
        value, count = entry.decode(raw)
        print(value)
    except UnicodeDecodeError as exc:
        print(str(exc))
for policy in ["ignore", "replace", "backslashreplace"]:
    value, count = entry.decode({bytes(range(256))!r}, policy)
    print(value.encode("utf-8"))
    print(count)
for name in ["cp424", "cp1251", "ascii", "iso8859-1"]:
    encoder = codecs.lookup(name)
    for sample in ["¢\\n", "a🙂€z", "🙂\\u0378\\U0001fae9", "中한"]:
        for policy in ["strict", "ignore", "replace", "backslashreplace", "xmlcharrefreplace", "namereplace", "no-such-handler"]:
            try:
                raw, count = encoder.encode(sample, policy)
                print(raw)
                print(count)
            except UnicodeEncodeError as exc:
                print(str(exc))
            except LookupError as exc:
                print(str(exc))
""".encode()


def output(command, env=None):
    child = subprocess.run(command, capture_output=True, timeout=60, env=env, check=True)
    if child.stderr:
        raise AssertionError(child.stderr)
    return child.stdout


def main():
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        raise SystemExit("run with CPython 3.14.5")
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "cpython-source", "result"):
        parser.add_argument("--" + arg, type=Path, required=True)
    args = parser.parse_args()
    for root in (SHARED / "upstream", args.cpython_source):
        verify_files(root, FILES)
    defined, text, missing = mappings(args.cpython_source)
    source = source_program(defined, text, missing)
    tree = ast.parse((args.cpython_source / TEST).read_bytes())
    cls, = [n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "SysModuleTest"]
    method, = [n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == "test_ioencoding"]
    popen = next(n.value for n in method.body if isinstance(n, ast.Assign) and isinstance(n.value, ast.Call)
                 and ast.unparse(n.value.func) == "subprocess.Popen")
    original = ast.literal_eval(popen.args[0].elts[2])
    observed = output([sys.executable, "-c", original], dict(os.environ, PYTHONIOENCODING="cp424"))
    assert observed == b"\x4a\x25"
    report = {"upstream_id": "test.test_sys.SysModuleTest.test_ioencoding", "whole_methods_added": 0,
              "files": FILES | {TABLE: TABLE_BLOB}, "defined_bytes": len(defined),
              "undefined_bytes": [b.hex() for b in missing], "source_sha256": hashlib.sha256(source).hexdigest(),
              "reference_child": {"source": original, "stdout_hex": observed.hex()}, "levels": {}}
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        script = work / "reference.py"
        script.write_bytes(source)
        expected = output([sys.executable, "-I", "-B", script])
        assert expected.splitlines()[:9] == [b"cp424"] * 5 + [b"True", b"218", b"True", b"218"]
        for level in (0, 3):
            tools = Toolchain(args.imre, args.runtime, args.clang, level)
            actual = output([tools.build(source, work)])
            if expected != actual:
                raise AssertionError((level, expected, actual))
            flags = ["--quiet", "--no-cache", "--runtime", str(tools.runtime.resolve()),
                     "--clang", str(tools.clang.resolve()), "--opt-level", str(level)]
            env = dict(os.environ, PYTHONIOENCODING="cp424", IMRE_ENCODED_FLAGS="\x1f".join(flags))
            native_child = output([tools.imre.resolve(), "-c", original], env)
            if native_child != observed:
                raise AssertionError((level, original, observed.hex(), native_child.hex()))
            report["levels"][str(level)] = {"stdout_sha256": hashlib.sha256(actual).hexdigest(),
                                            "bytes": len(actual), "lines": len(actual.splitlines()),
                                            "original_child": {"stdout_hex": native_child.hex(), "status": 0,
                                                "outcome": "pass"}}
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
