"""Run unchanged formatting methods with exact fixture values in native JSON calls."""

import argparse
import ast
import hashlib
import json
import math
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from reference import BENCH, COMMIT, routine, verify
sys.path.insert(0, str(BENCH))
from suite_inventory import cases, load_suite
from suite_native import Toolchain, run_in_fixtures


METHODS = {
    "test_indent.TestIndent.test_indent": 8, "test_separators.TestSeparators.test_separators": 4,
    "test_separators.TestSeparators.test_illegal_separators": 3,
    "test_encode_basestring_ascii.TestEncodeBasestringAscii.test_sorted_dict": 1,
    "test_dump.TestCDump.test_large_list": 1,
}


def literal(value):
    """Transfer exact builtin values as Python literals; never serialize operands with JSON."""
    ty = type(value)
    if ty in (str, bytes, int, bool) or value is None:
        return ascii(value)
    if ty is float:
        return repr(value) if math.isfinite(value) else f"float({str(value)!r})"
    if ty is list:
        if len(value) > 8 and all(type(v) is int and v == value[0] for v in value):
            return f"[{value[0]}] * {len(value)}"
        return "[" + ", ".join(map(literal, value)) + "]"
    if ty is tuple:
        return "(" + "".join(literal(x) + ", " for x in value) + ")"
    if ty is dict:
        return "{" + ", ".join(f"{literal(k)}: {literal(v)}" for k, v in value.items()) + "}"
    raise RuntimeError(f"no native fixture literal for {ty.__name__}")


class Native:
    def __init__(self, tools, directory):
        self.tools, self.directory = tools, directory
        self.cache = {}
        self.calls = []

    def operation(self, method, value, options):
        arguments = [literal(value), *(f"{name}={literal(v)}" for name, v in options.items())]
        source = f"""import json

try:
    result = json.{method}({", ".join(arguments)})
    print("OK")
    print(repr(result))
except TypeError as exc:
    print("TypeError")
    print(str(exc))
except ValueError as exc:
    print("ValueError")
    print(str(exc))
""".encode()
        key = hashlib.sha256(source).hexdigest()
        if key not in self.cache:
            work = self.directory / key
            work.mkdir()
            self.cache[key] = self.tools.build(source, work)
        run = subprocess.run([self.cache[key]], capture_output=True, text=True, timeout=60)
        if run.returncode or run.stderr:
            raise RuntimeError(f"native program {run.returncode}: {run.stderr}")
        tag, sep, payload = run.stdout.removesuffix("\n").partition("\n")
        if not sep or not run.stdout.endswith("\n"):
            raise RuntimeError("incomplete native result")
        self.calls.append({"method": method, "source_sha256": key, "status": tag})
        if tag in ("TypeError", "ValueError"):
            raise {"TypeError": TypeError, "ValueError": ValueError}[tag](payload)
        if tag != "OK":
            raise RuntimeError(tag)
        try:
            return ast.literal_eval(payload)
        except (ValueError, SyntaxError) as exc:
            raise RuntimeError("cannot reconstruct the native result") from exc

    def dumps(self, value, **kwargs):
        return self.operation("dumps", value, kwargs)

    def loads(self, value):
        return self.operation("loads", value, {})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, required=True, type=Path)
    args = parser.parse_args()
    verify()
    report = {"cpython_commit": COMMIT, "fixture": "original CPython fixtures, body and assertions",
              "operation": "Imre standard json", "levels": {}}
    ok = True
    for level in (0, 3):
        with tempfile.TemporaryDirectory() as temp:
            native = Native(Toolchain(args.imre, args.runtime, args.clang, level), Path(temp))
            executions = {}
            for case in cases(load_suite("test.test_json")):
                source = routine(case).removeprefix("test.test_json.")
                if source not in METHODS:
                    continue

                def execute(case, original, source=source):
                    start = len(native.calls)
                    case.json, case.loads, case.dumps = native, native.loads, native.dumps
                    original()
                    operations = native.calls[start:]
                    if len(operations) != METHODS[source]:
                        raise AssertionError((source, len(operations), METHODS[source]))
                    return {"source": source, "execution": "host-driven", "native_calls": operations}
                executions[case.id()] = execute
            evidence, failures, skips = run_in_fixtures(load_suite("test.test_json"), executions)
            if failures or skips or executions.keys() != evidence.keys():
                print(level, failures, skips, file=sys.stderr)
                ok = False
            report["levels"][str(level)] = {"ids": evidence, "failures": failures, "skips": skips}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(not ok)


if __name__ == "__main__":
    sys.exit(main())
