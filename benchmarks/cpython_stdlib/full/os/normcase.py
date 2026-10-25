"""Exercise the two CPython POSIX normcase tests and fnmatch's import-time policy."""

import argparse
import ast
from contextlib import chdir
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import NativeCompileError, Toolchain, method_bytes, run_in_fixtures

COMMIT = "5607950ef232dad16d75c0cf53101d9649d89115"
FILES = {
    "Lib/test/test_fnmatch.py": "5daaf3b3fddb9eae3eb87bc420b8508b967146d2",
    "Lib/test/test_genericpath.py": "1a44cedcd360b11f2924bf7effca09a04c5e259a",
    "Lib/test/test_posixpath.py": "21f06712548d888ddf999ae127881420011d881e",
    "Lib/test/support/os_helper.py": "2c45fe2369ec36b574258e701bfc078114c5499d",
}
COMMON = "test.test_posixpath.PosixCommonTest.test_normcase"
PATHLIKE = "test.test_posixpath.PathLikeTests.test_path_normcase"

# The host supplies FakePath.path and file_name separately. The fixture's
# string case of FakePath.__fspath__ and both public operations execute in Imre.
PATH_FIXTURE = """import posixpath
import sys

class FakePath:
    path: str
    def __init__(self, path: str) -> None:
        self.path = path
    def __fspath__(self) -> str:
        return self.path

class PathModule:
    def normcase(self, path: str | FakePath) -> str:
        return posixpath.normcase(path)

class NativeCase:
    path: PathModule
    file_path: FakePath
    file_name: str
    def __init__(self, path: str, filename: str) -> None:
        self.path = PathModule()
        self.file_path = FakePath(path)
        self.file_name = filename
    def assertEqual(self, first: str, second: str) -> None:
        assert first == second
"""
COMMON_FIXTURE = """import posixpath
import unittest
class Module:
    def normcase(self, value: str | bytes) -> str | bytes:
        return posixpath.normcase(value)
class NativeCase(unittest.TestCase):
    pathmodule: Module
    def __init__(self) -> None:
        super().__init__()
        self.pathmodule = Module()
"""


def compile_source(tools, work, name, source):
    directory = work / name
    directory.mkdir()
    return tools.build(source, directory)


def run(binary, *args):
    return subprocess.run([binary, *args], capture_output=True, text=True,
                          check=True, timeout=30).stdout


def sha(data):
    return hashlib.sha256(data).hexdigest()


def diagnostic(exc, work):
    return str(exc).replace(str(work), "<fixture>").strip().splitlines()[-1]


def fnmatch_policy(tools, work, test_lib):
    path = test_lib / "test_fnmatch.py"
    source = path.read_bytes()
    names = {"IGNORECASE", "NORMSEP"}
    lines = source.splitlines(keepends=True)
    definitions = [b"".join(lines[node.lineno - 1:node.end_lineno])
                   for node in ast.parse(source).body if isinstance(node, ast.Assign)
                   and isinstance(node.targets[0], ast.Name) and node.targets[0].id in names]
    if len(definitions) != 2:
        raise ValueError("missing original fnmatch platform assignments")
    program = b"import os\n" + b"".join(definitions) + b"print(IGNORECASE)\nprint(NORMSEP)\n"
    actual = run(compile_source(tools, work, "fnmatch-policy", program))
    expected = subprocess.check_output([sys.executable, "-c", program], text=True)
    if actual != expected or actual != "False\nFalse\n":
        raise AssertionError((actual, expected))
    return {"execution": "native import-time assignments", "IGNORECASE": False, "NORMSEP": False,
            "source_sha256": sha(b"".join(definitions))}


def pathlike_adapter(tools, work, test_lib):
    path = test_lib / "test_posixpath.py"
    method = method_bytes(path, "PathLikeTests", "test_path_normcase")
    helper = method_bytes(path, "PathLikeTests", "assertPathEqual")
    entry = b"\nNativeCase(sys.argv[1], sys.argv[2]).test_path_normcase()\n"
    binary = compile_source(tools, work, "pathlike", PATH_FIXTURE.encode() + helper + method + entry)

    def execute(case, original):
        from test.support.os_helper import FakePath
        if type(case.file_path) is not FakePath or type(case.file_path.path) is not str:
            raise ValueError("unexpected CPython fixture path")
        run(binary, case.file_path.path, case.file_name)
        return {"classification": "native-body-host-fixture", "body": "imre",
                "assertions": "imre", "operation": "imre", "fixture": "cpython",
                "method_sha256": sha(method), "helper_sha256": sha(helper),
                "calls": 2}
    return execute


def common_adapter(tools, work, test_lib):
    method = method_bytes(test_lib / "test_genericpath.py", "CommonTest", "test_normcase")
    try:
        binary = compile_source(tools, work, "common-whole", COMMON_FIXTURE.encode() + method
                                + b"\nNativeCase().test_normcase()\n")
    except NativeCompileError as exc:
        blocker = diagnostic(exc, work)
    else:
        def whole(case, original):
            run(binary)
            return {"classification": "native-body-host-fixture", "body": "imre",
                    "assertions": "imre", "operation": "imre", "fixture": "cpython",
                    "method_sha256": sha(method)}
        return whole

    def execute(case, original):
        operations = []
        cache = {}

        def normcase(value):
            expression = repr(value)
            annotation = ": list[int]" if isinstance(value, list) else ""
            key = (type(value), expression)
            if key not in cache:
                program = (
                    f"import posixpath\nvalue{annotation} = {expression}\ntry:\n"
                    "    result = posixpath.normcase(value)  # ty: ignore[no-matching-overload]\n"
                    "    print('value ' + repr(result))\nexcept TypeError as error:\n"
                    "    print('TypeError ' + str(error))\n"
                ).encode()
                try:
                    cache[key] = compile_source(tools, work, f"common-{len(cache)}", program)
                except NativeCompileError as exc:
                    if type(value) is not bytearray:
                        raise
                    cache[key] = diagnostic(exc, work)
            native = cache[key]
            if isinstance(native, str):
                # Preserve the unchanged method, recording the sole host call
                # explicitly while Imre has no native bytearray construction.
                operations.append({"input": type(value).__name__, "operation": "cpython", "blocker": native})
                return os.path.normcase(value)
            output = run(native).rstrip("\n")
            kind, _, data = output.partition(" ")
            operations.append({"input": type(value).__name__, "operation": "imre"})
            if kind == "TypeError":
                raise TypeError(data)
            if kind != "value":
                raise ValueError(output)
            return ast.literal_eval(data)

        with patch.object(case, "pathmodule", SimpleNamespace(normcase=normcase)):
            original()
        host = any(op["operation"] == "cpython" for op in operations)
        return {"classification": "mixed" if host else "host-driven-native-operation",
                "body": "cpython", "assertions": "cpython", "operation": "mixed" if host else "imre",
                "fixture": "cpython", "method_sha256": sha(method),
                "whole_compile_blocker": blocker, "calls": operations}
    return execute


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "cpython-source", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or os.name != "posix":
        parser.error("requires CPython 3.14.5 on POSIX")
    source = args.cpython_source.resolve()
    result = args.result.resolve()
    tools = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), args.opt_level)
    verify_files(source, FILES)
    vendored = {k: v for k, v in FILES.items() if k.endswith(("test_genericpath.py", "test_posixpath.py"))}
    verify_files(STDLIB / "upstream", {**vendored, "LICENSE": "20cf39097c68baa17cc566b64e76d34ebf034044"})
    sys.path.insert(0, str(source / "Lib"))
    verify_interpreter_tests({"files": FILES})
    with tempfile.TemporaryDirectory() as directory, chdir(directory):
        work = Path(directory)
        suite = load_suite("test.test_posixpath")
        selected = sorted(case.id() for case in cases(suite) if "normcase" in case.id())
        if selected != sorted((COMMON, PATHLIKE)):
            raise ValueError(f"unexpected release normcase IDs: {selected}")
        baseline, failures, skips = run_in_fixtures(suite, {key: lambda case, original: original() for key in selected})
        if set(baseline) != set(selected) or failures or skips:
            raise AssertionError((baseline, failures, skips))
        test_lib = source / "Lib/test"
        records, failures, skips = run_in_fixtures(
            load_suite("test.test_posixpath"),
            {COMMON: common_adapter(tools, work, test_lib), PATHLIKE: pathlike_adapter(tools, work, test_lib)})
        if failures or skips or set(records) != set(selected):
            raise AssertionError((records, failures, skips))
        policy = fnmatch_policy(tools, work, test_lib)
        regression = HERE.parents[3] / "tests/fixtures/paths/posix_normcase.py"
        binary = compile_source(tools, work, "identity-and-errors", regression.read_bytes())
        reference = subprocess.check_output([sys.executable, regression], text=True)
        if run(binary) != reference:
            raise AssertionError("path identity, protocol exception, or argument evaluation differs")
    report = {"cpython_commit": COMMIT, "platform": sys.platform, "optimization": args.opt_level,
              "format": "posix-normcase-v1", "selected_posixpath_occurrences": len(selected), "reference": {key: "pass" for key in selected},
              "occurrences": records, "fnmatch_policy": policy}
    result.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
