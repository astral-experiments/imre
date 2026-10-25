"""Run unmodified CPython POSIX lexical tests with native Imre for each path operation."""
import argparse
import ast
from contextlib import chdir
from functools import partial
import json
import os
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

LEXICAL = {"join": 19, "basename": 10, "dirname": 10, "splitext": 144, "normpath": 100}
COMMON = {"abspath": 7, "abspath_issue3426": 10, "normpath_issue5827": 5, "normpath_issue106242": 5}
OPERATIONS = (*LEXICAL, "abspath")


def driver_source():
    # Absolute filenames carry raw bytes and embedded NUL while the child
    # inherits the working directory chosen by the upstream fixture.
    source = """import os.path
import sys
from pathlib import Path

def arg(i: int) -> bytes:
    with open(sys.argv[2 + i], "rb") as stream:
        return stream.read()

selector = sys.argv[1]
"""
    for name in OPERATIONS:
        for count in range(1, 5 if name == "join" else 2):
            for kind in ("text", "bytes"):
                suffix = '.decode("utf-8")' if kind == "text" else ''
                args = [f"arg({i}){suffix}" for i in range(count)]
                key = f"{name}-{kind}-{count}"
                call = f'print(repr(os.path.{name}({", ".join(args)})))'
                source += f'if selector == "{key}":\n'
                if name == "abspath":
                    source += f'    try:\n        {call}\n    except FileNotFoundError as error:\n'
                    source += '        print(repr(("FileNotFoundError", str(error))))\n'
                else:
                    source += f"    {call}\n"
    source += '''if selector.startswith("common-"):
    try:
        if selector == "common-none":
            print(repr(os.path.commonpath(None)))  # ty: ignore[no-matching-overload]
        elif selector == "common-iter":
            print(repr(os.path.commonpath(iter([]))))
        elif selector == "common-paths":
            from_marker = Path("a/b")
            named_paths = [from_marker, Path("a/c")]
            print(repr((os.path.commonpath(named_paths), os.path.commonpath((from_marker, "a/d")), os.devnull)))
        else:
            paths: list[str | bytes] = []
            for i in range(2, len(sys.argv), 2):
                with open(sys.argv[i + 1], "rb") as file:
                    raw = file.read()
                if sys.argv[i] == "bytes":
                    paths.append(raw)
                else:
                    paths.append(raw.decode())
            print(repr(os.path.commonpath(paths=paths)))  # ty: ignore[no-matching-overload]
    except ValueError as error:
        print(repr(("ValueError", str(error))))
    except TypeError as error:
        print(repr(("TypeError", str(error))))
'''
    return source


def invoke_commonpath(program, calls, paths):
    if paths is None:
        mode, args = "none", ()
    elif type(paths) in (list, tuple):
        mode, args = "sequence", paths
    else:
        # CPython's original method also supplies iter([]); execute that expression in the native child.
        assert list(paths) == []
        mode, args = "iter", ()
    command = [program, "common-" + mode]
    for i, path in enumerate(args):
        file = program.parent / str(i)
        file.write_bytes(path if isinstance(path, bytes) else path.encode("utf-8"))
        command.extend(["bytes" if isinstance(path, bytes) else "str", file])
    result = subprocess.run(command, check=True, capture_output=True, text=True, timeout=30)
    assert not result.stderr, result
    value = ast.literal_eval(result.stdout)
    calls.append("commonpath")
    if isinstance(value, tuple):
        kind, message = value
        raise {"TypeError": TypeError, "ValueError": ValueError}[kind](message)
    return value


def invoke(program, calls, name, *args):
    kind = "bytes" if isinstance(args[0], bytes) else "text"
    if type(args[0]) not in (str, bytes) or not all(type(arg) is type(args[0]) for arg in args):
        raise AssertionError("the native driver accepts only exact str or bytes")
    command = [program, f"{name}-{kind}-{len(args)}"]
    for i, arg in enumerate(args):
        path = program.parent / str(i)
        path.write_bytes(arg if kind == "bytes" else arg.encode("utf-8"))
        command.append(path)
    run = subprocess.run(command, capture_output=True, text=True, timeout=30, check=True)
    if run.stderr:
        raise AssertionError(run.stderr)
    result = ast.literal_eval(run.stdout)
    calls.append(name)
    return result


def cwd_controls(program, undecodable):
    # Check exact values and errors beyond the upstream type assertions.
    def compare(*inputs):
        calls = []
        for path in inputs:
            try:
                expected = os.path.abspath(path)
            except FileNotFoundError as error:
                expected = ("FileNotFoundError", str(error))
            actual = invoke(program, calls, "abspath", path)
            if actual != expected:
                raise AssertionError(f"abspath({path!r}): {actual!r} != {expected!r}")
        return len(calls)

    controls = {}
    if undecodable:
        os.mkdir(undecodable)
        try:
            with chdir(undecodable):
                controls["undecodable_cwd"] = compare(b"\xff", b".", "..", "../known", "/absolute")
        finally:
            os.rmdir(undecodable)
    deleted = program.parent / "deleted"
    deleted.mkdir()
    with chdir(deleted):
        deleted.rmdir()
        controls["deleted_cwd"] = compare("relative", b"relative", "//a/../b", b"/a/../b")
    return controls


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + option, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    parser.add_argument("--opt-level", choices=("0", "3"), default="3")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or os.name != "posix":
        parser.error("the original POSIX unittest requires CPython 3.14.5")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    vendored = HERE / "upstream/Lib/test"
    test.__path__.insert(0, str(vendored))
    from test import test_posixpath as suite
    for module in (suite, suite.test_genericpath):
        if Path(module.__file__).resolve().parent != vendored:
            raise RuntimeError(f"{module.__name__} must come from the pinned vendored files")
    report = {"cpython_commit": verify_upstream(), "host": "unittest assertions and filesystem setup",
              "native": "os.path operations", "tests": {}}
    with tempfile.TemporaryDirectory() as directory:
        work = Path(directory)
        source, program = work / "driver.py", work / "driver"
        source.write_text(driver_source())
        subprocess.run([args.imre.resolve(), "build", source, "-o", program, "--no-cache",
                        "--opt-level", args.opt_level, "--runtime", args.runtime.resolve(),
                        "--clang", args.clang.resolve()], cwd=work, check=True, timeout=120)
        cases = [(suite.PosixPathTest("test_" + name), name, count) for name, count in LEXICAL.items()]
        cases.append((suite.PosixPathTest("test_commonpath"), "commonpath", 59))
        cases += [(suite.PosixCommonTest("test_" + name), name.split("_")[0], count)
                  for name, count in COMMON.items()]
        excluded = {suite.PathLikeTests("test_path_" + name).id(): "requires a user-defined __fspath__"
                    for name in ("normpath", "abspath")}
        nonascii = suite.PosixCommonTest("test_nonascii_abspath")
        undecodable = (suite.os_helper.TESTFN_UNDECODABLE if not suite.support.is_apple
                       and sys.platform not in ("emscripten", "wasi") else None)
        if undecodable:
            excluded[nonascii.id()] = "relative str results contain unsupported surrogateescape code points"
        elif suite.os_helper.TESTFN_NONASCII:
            cases.append((nonascii, "abspath", 7))
        else:
            excluded[nonascii.id()] = "upstream fixture requires a non-ASCII directory"
        report["excluded"] = excluded
        with chdir(work):
            for case, name, expected in cases:
                calls = []
                proxy = SimpleNamespace(lexists=os.path.lexists, commonpath=partial(invoke_commonpath, program, calls),
                                        **{op: partial(invoke, program, calls, op) for op in OPERATIONS})
                # PosixPathTest uses the module global; inherited CommonTest methods
                # use the instance attribute. Host setup still checks real files.
                case.pathmodule = proxy
                with mock.patch.object(suite, "posixpath", proxy):
                    result = unittest.TestResult()
                    case.run(result)
                passed = (result.wasSuccessful() and result.testsRun == 1 and not result.skipped
                          and calls == [name] * expected)
                report["tests"][case.id()] = {"status": "pass" if passed else "fail", "native_calls": len(calls),
                                             "diagnostics": [s for _, s in result.errors + result.failures + result.skipped]}
                print(report["tests"][case.id()]["status"], case.id(), len(calls))
            report["cwd_controls"] = cwd_controls(program, undecodable)
            controls = [["/x/../a", "/x/../b"], [b"/\xff/a", b"/\xff/b"], ["a\0/b", "a\0/c"],
                        [b"/x", b"relative", "text"]]
            for inputs in controls:
                try:
                    expected = os.path.commonpath(inputs)
                except (ValueError, TypeError) as error:
                    try:
                        invoke_commonpath(program, [], inputs)
                    except type(error) as actual:
                        assert str(actual) == str(error)
                    else:
                        raise AssertionError(inputs)
                else:
                    assert invoke_commonpath(program, [], inputs) == expected
            run = subprocess.run([program, "common-paths"], check=True, text=True, capture_output=True)
            assert not run.stderr and ast.literal_eval(run.stdout) == ("a", "a", os.devnull)
            report["commonpath_controls"] = len(controls) + 1
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(any(value["status"] != "pass" for value in report["tests"].values()))


if __name__ == "__main__":
    sys.exit(main())
