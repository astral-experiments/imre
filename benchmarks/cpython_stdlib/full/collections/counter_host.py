"""Run the original Counter subtraction routine with host unittest and native operations."""

import argparse
import ast
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import load_suite, verify_files
from suite_native import Toolchain, method_bytes, run_in_fixtures


class Native:
    """Replay native state up to each observation. The host retains source, never counts."""

    def __init__(self, tools, directory):
        self.tools, self.directory = tools, directory
        self.history = ["from collections import Counter"]
        self.calls = {}
        self.handles = 0

    def run(self, label, statements):
        self.calls[label] = self.calls.get(label, 0) + 1
        source = "\n".join([*self.history, *statements, ""])
        binary = self.tools.build(source.encode(), self.directory)
        result = subprocess.run([binary], text=True, capture_output=True, check=True, timeout=30)
        if result.stderr:
            raise AssertionError(result.stderr)
        return result.stdout.splitlines()

    def mutate(self, label, statement):
        # Reconstruct successful mutations in order. Preserve any changes before
        # a caught native TypeError if a later assertion observes its receiver.
        output = self.run(label, ["try:", "    " + statement, "except TypeError as exc:",
                                  '    print("error")', "    print(str(exc))", "else:",
                                  '    print("ok")'])
        if output == ["ok"]:
            self.history.append(statement)
        elif len(output) == 2 and output[0] == "error":
            self.history.extend(["try:", "    " + statement, "except TypeError:", "    pass"])
            raise TypeError(output[1])
        else:
            raise AssertionError(output)

    def __call__(self, /, *args, **kwargs):
        name = f"counter_{self.handles}"
        self.handles += 1
        self.mutate("Counter", f"{name} = Counter({arguments(args, kwargs)})")
        return Handle(self, name)

    def subtract(self, /, *args, **kwargs):
        self.mutate("Counter.subtract (unbound)", f"Counter.subtract({arguments(args, kwargs)})")


def value(item):
    if isinstance(item, Handle):
        return item.name
    if item is None or type(item) in (int, str):
        return repr(item)
    if type(item) is dict and all(type(k) is str and type(v) is int for k, v in item.items()):
        return repr(item)
    raise TypeError(f"unsupported native argument transport: {type(item).__name__}")


def arguments(args, kwargs):
    return ", ".join([*(value(arg) for arg in args), *(f"{k}={value(v)}" for k, v in kwargs.items())])


class Handle:
    def __init__(self, native, name):
        self.native, self.name = native, name

    def subtract(self, /, *args, **kwargs):
        self.native.mutate("subtract", f"{self.name}.subtract({arguments(args, kwargs)})")

    def __eq__(self, other):
        if not isinstance(other, Handle):
            return NotImplemented
        output = self.native.run("Counter equality", [f"print({self.name} == {other.name})"])
        if output not in (["True"], ["False"]):
            raise AssertionError(output)
        return output == ["True"]

    def items(self):
        output = self.native.run("items iteration", [f"for key, count in {self.name}.items():",
                                                     "    print(repr(key))", "    print(count)"])
        words = iter(output)
        return [(ast.literal_eval(key), int(count)) for key, count in zip(words, words, strict=True)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--opt-level", choices=(0, 3), type=int, required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use a test-enabled CPython 3.14.5")
    from test import test_collections

    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_files(Path(test_collections.__file__).resolve().parents[2], manifest["files"])
    tools = Toolchain(args.imre, args.runtime, args.clang, args.opt_level)
    case_id = "test.test_collections.TestCounter.test_subtract"
    source = HERE / "upstream/Lib/test/test_collections.py"
    with tempfile.TemporaryDirectory() as work:
        def execute(case, original):
            native = Native(tools, Path(work))
            with patch.object(test_collections, "Counter", native):
                original()
            body = method_bytes(source, "TestCounter", case._testMethodName)
            return {"execution": "host-driven-native", "body": "CPython", "assertions": "CPython",
                    "fixture_execution": "CPython module/class/instance hooks",
                    "operation": "Imre for all Counter operations, including raised TypeErrors",
                    "native_calls": dict(native.calls), "method_sha256": hashlib.sha256(body).hexdigest()}
        evidence, failures, skips = run_in_fixtures(load_suite("test.test_collections"), {case_id: execute})
    args.result.write_text(json.dumps({"cpython_commit": manifest["commit"], "opt_level": args.opt_level,
                                      "tests": evidence, "failures": failures, "skips": skips}, indent=2) + "\n")
    return int(bool(failures or skips) or case_id not in evidence)


if __name__ == "__main__":
    sys.exit(main())
