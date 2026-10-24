"""Run original CPython JSON value tests against a standalone native Relar driver."""
import argparse
import ast
import importlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from native_json import HERE, sha256, vendored_decode
from upstream import verify_upstream


# Expected native operation counts guard against vacuous skips or a host JSON call.
METHODS = {
    "Decode": {"empty_objects": 3, "string_with_utf8_bom": 4},
    "Dump": {"dump": 1, "dumps": 1},
    "Float": {"floats": 15, "ints": 6, "out_of_range": 2, "allow_nan": 9},
    "Indent": {"indent0": 4},
    "Unicode": {"encoding3": 1, "encoding4": 1, "encoding5": 1, "encoding6": 1,
                "encoding7": 1, "ascii_non_printable_encode": 2, "big_unicode_encode": 2,
                "big_unicode_decode": 2, "unicode_preservation": 3, "bytes_encode": 2},
}


def unpack_native_repr(payload):
    class NumberNames(ast.NodeTransformer):
        def visit_Name(self, node):
            if node.id not in {"inf", "nan"}:
                raise ValueError(f"unexpected name in native value: {node.id}")
            # ast.literal_eval omits the standard spellings for these two floats.
            return ast.Constant(value=float(node.id))
    return ast.literal_eval(NumberNames().visit(ast.parse(payload, mode="eval")))


class Native:
    def __init__(self, program, directory, error):
        self.program = program
        self.directory = directory
        self.error = error
        self.calls = []

    def invoke(self, method, shape, data, ensure_ascii=True, allow_nan=True, indent=None):
        (self.directory / "input.bin").write_bytes(data)
        (self.directory / "output.txt").unlink(missing_ok=True)
        args = [method, shape, str(int(ensure_ascii)), str(int(allow_nan)), str(indent)]
        run = subprocess.run([self.program, *args], cwd=self.directory, capture_output=True,
                             timeout=10)
        if run.returncode or run.stderr:
            raise RuntimeError(f"native {method} failed: {run.returncode}, {run.stderr!r}")
        tag, sep, payload = run.stdout.decode().removesuffix("\n").partition("\n")
        self.calls.append({"method": method, "shape": shape, "status": tag})
        if not sep or not run.stdout.endswith(b"\n"):
            raise RuntimeError("incomplete native JSON response")
        if tag == "JSONDecodeError":
            # The selected upstream method checks only the native category and message.
            error = self.error.__new__(self.error)
            ValueError.__init__(error, payload)
            raise error
        if tag in {"TypeError", "ValueError"}:
            raise {"TypeError": TypeError, "ValueError": ValueError}[tag](payload)
        if tag != "OK":
            raise RuntimeError(f"unexpected native result {tag!r}")
        return payload

    def loads(self, document):
        return unpack_native_repr(self.invoke("loads", "str", document.encode()))

    def load(self, fp):
        return unpack_native_repr(self.invoke("load", "file", fp.read().encode()))

    @staticmethod
    def encode_input(value):
        if type(value) is dict and all(type(k) is type(v) is int for k, v in value.items()):
            return "dict", "".join(f"{k}\n{v}\n" for k, v in value.items()).encode()
        prefix = ""
        if type(value) is list:
            if len(value) != 1:
                raise ValueError("the typed driver requires a one-element list")
            prefix, value = "list-", value[0]
        if type(value) in {int, float, str, bytes}:
            data = value if type(value) is bytes else str(value).encode()
            return prefix + type(value).__name__, data
        raise ValueError(f"unsupported native argument {type(value)}")

    def dumps(self, value, *, ensure_ascii=True, allow_nan=True, indent=None):
        shape, data = self.encode_input(value)
        if indent is not None and shape != "dict":
            raise ValueError("the typed driver tests indentation on dictionaries")
        return self.invoke("dumps", shape, data, ensure_ascii, allow_nan, indent)

    def dump(self, value, fp, *, indent=None):
        shape, data = self.encode_input(value)
        if shape != "dict" or self.invoke("dump", shape, data, indent=indent) != "None":
            raise RuntimeError("native json.dump must write the dictionary and return None")
        fp.write((self.directory / "output.txt").read_bytes().decode())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("relar", "runtime", "clang", "result"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use CPython 3.14.5 for the original unittest assertions")
    tests = {}
    report = {"cpython_commit": verify_upstream(), "assertions": "unmodified host unittest",
              "json_operations": "native Relar", "behavioral_methods": sum(map(len, METHODS.values())),
              "tests": tests}
    try:
        vendored_decode(args.cpython_source)
        driver = HERE / "json_values_driver.py"
        report["sha256"] = {k: sha256(p) for k, p in (("driver", driver), ("relar", args.relar),
            ("runtime", args.runtime), ("clang", args.clang))}
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            program, ir = directory / "driver", directory / "driver.ll"
            subprocess.run([args.relar.resolve(), "build", driver, "-o", program, "--no-cache",
                            "--opt-level", "3", "--clang", args.clang.resolve(),
                            "--runtime", args.runtime.resolve(), "--emit-llvm", ir], cwd=tmp,
                           check=True, timeout=120)
            generated = ir.read_text()
            for call in ("ptr @relar_json_loads(", "ptr @relar_json_dumps(", "i32 @relar_json_dump("):
                if "call " + call not in generated:
                    raise RuntimeError(f"native driver omits {call}")
            report["sha256"]["executable"] = sha256(program)
            for group, methods in METHODS.items():
                name = "test_" + group.lower()
                module = importlib.import_module("test.test_json." + name)
                if Path(module.__file__).resolve() != HERE / "upstream/Lib/test/test_json" / (name + ".py"):
                    raise RuntimeError("JSON methods must come from the verified vendor tree")
                for backend in ("Py", "C"):
                    cls = getattr(module, f"Test{backend}{group}")
                    for method, count in methods.items():
                        case = cls("test_" + method)
                        native = Native(program, directory, case.JSONDecodeError)
                        case.json, case.dumps, case.loads = native, native.dumps, native.loads
                        result = unittest.TestResult()
                        case.run(result)
                        ok = (result.testsRun == 1 and result.wasSuccessful() and not result.skipped
                              and len(native.calls) == count)
                        tests[case.id()] = {"status": "pass" if ok else "fail", "native_calls": native.calls,
                            "diagnostics": [s for _, s in [*result.errors, *result.failures, *result.skipped]]}
                        print(tests[case.id()]["status"], case.id())
    except (ImportError, OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        report["error"] = str(error)
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int("error" in report or any(t["status"] != "pass" for t in tests.values()))


if __name__ == "__main__":
    sys.exit(main())
