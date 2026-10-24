"""Run unmodified CPython POSIX lexical tests with native Imre for each path operation."""
import argparse
import ast
from functools import partial
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from upstream import verify_upstream

METHODS = {"join": 19, "basename": 10, "dirname": 10, "splitext": 144}


def driver_source():
    # Files transport arbitrary bytes independently of argv encoding.
    source = """import os.path
import sys

def arg(i: int) -> bytes:
    with open(str(i), "rb") as stream:
        return stream.read()

selector = sys.argv[1]
"""
    for name in METHODS:
        for count in range(1, 5 if name == "join" else 2):
            for kind in ("text", "bytes"):
                args = [f"arg({i})" for i in range(count)]
                if kind == "text":
                    args = [f"sys.argv[{2 + i}]" for i in range(count)]
                key = f"{name}-{kind}-{count}"
                source += f'if selector == "{key}":\n    print(repr(os.path.{name}({", ".join(args)})))\n'
    return source


def invoke(program, calls, name, *args):
    kind = "bytes" if isinstance(args[0], bytes) else "text"
    if not all(type(arg) is type(args[0]) for arg in args):
        raise AssertionError("unexpected mixed input in the selected upstream tests")
    command = [program, f"{name}-{kind}-{len(args)}"]
    if kind == "bytes":
        for i, arg in enumerate(args):
            (program.parent / str(i)).write_bytes(arg)
    else:
        command.extend(args)
    run = subprocess.run(command, cwd=program.parent,
                         capture_output=True, text=True, timeout=30, check=True)
    if run.stderr:
        raise AssertionError(run.stderr)
    result = ast.literal_eval(run.stdout)
    calls.append(name)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + option, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("the original unittest requires CPython 3.14.5")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    vendored = HERE / "upstream/Lib/test"
    test.__path__.insert(0, str(vendored))
    from test import test_posixpath as suite
    if Path(suite.__file__).resolve() != vendored / "test_posixpath.py":
        raise RuntimeError("the selected posixpath tests must come from the pinned vendored file")
    report = {"cpython_commit": verify_upstream(), "host": "unittest assertions and filesystem setup",
              "native": "os.path operations", "tests": {}}
    with tempfile.TemporaryDirectory() as directory:
        work = Path(directory)
        source, program = work / "driver.py", work / "driver"
        source.write_text(driver_source())
        subprocess.run([args.imre.resolve(), "build", source, "-o", program, "--no-cache",
                        "--opt-level", "3", "--runtime", args.runtime.resolve(),
                        "--clang", args.clang.resolve()], cwd=work, check=True, timeout=120)
        for name, expected in METHODS.items():
            calls = []
            case = suite.PosixPathTest("test_" + name)
            # setUp checks the absence of its sentinel on the host. Every tested
            # lexical function, including calls made by splitextTest, goes native.
            proxy = SimpleNamespace(lexists=suite.os.path.lexists,
                                    **{op: partial(invoke, program, calls, op) for op in METHODS})
            with mock.patch.object(suite, "posixpath", proxy):
                result = unittest.TestResult()
                case.run(result)
            passed = (result.wasSuccessful() and result.testsRun == 1 and not result.skipped
                      and calls == [name] * expected)
            report["tests"][case.id()] = {"status": "pass" if passed else "fail", "native_calls": len(calls),
                                         "diagnostics": [s for _, s in result.errors + result.failures + result.skipped]}
            print(report["tests"][case.id()]["status"], case.id(), len(calls))
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(any(value["status"] != "pass" for value in report["tests"].values()))


if __name__ == "__main__":
    sys.exit(main())
