"""Run CPython's original shutil.which filesystem fixtures against native Imre."""
import argparse
import ast
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from upstream import verify_upstream

OMITTED = object()


def driver_source():
    source = """import shutil
import sys

def raw(filename: str) -> bytes:
    with open(filename, "rb") as stream:
        return stream.read()

selector = sys.argv[1]
"""
    for cmd_bytes in (False, True):
        cmd = "raw(sys.argv[2])" if cmd_bytes else "sys.argv[2]"
        for path_kind in ("env", "text", "bytes"):
            for has_mode in (False, True):
                args = [cmd]
                if has_mode:
                    args.append("mode=int(sys.argv[4])")
                if path_kind != "env":
                    args.append("path=" + ("raw(sys.argv[3])" if path_kind == "bytes" else "sys.argv[3]"))
                key = f"{int(cmd_bytes)}-{path_kind}-{int(has_mode)}"
                source += f'if selector == "{key}":\n    print(repr(shutil.which({", ".join(args)})))\n'
    return source


def invoke(program, cmd, mode=OMITTED, path=None):
    def argument(value, filename):
        if isinstance(value, bytes):
            target = program.parent / filename
            target.write_bytes(value)
            return target
        return value
    path_kind = "env" if path is None else "bytes" if isinstance(path, bytes) else "text"
    key = f"{int(isinstance(cmd, bytes))}-{path_kind}-{int(mode is not OMITTED)}"
    run = subprocess.run([program, key, argument(cmd, "cmd.bin"), argument(path, "path.bin") if path is not None else "",
                          str(mode) if mode is not OMITTED else ""],
                         capture_output=True, text=True, timeout=30, check=True)
    if run.stderr:
        raise AssertionError(run.stderr)
    return ast.literal_eval(run.stdout)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for opt in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + opt, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or os.name != "posix":
        parser.error("the original POSIX tests require CPython 3.14.5")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    vendored = HERE / "upstream/Lib/test"
    test.__path__.insert(0, str(vendored))
    from test import test_shutil as suite
    if Path(suite.__file__).resolve() != vendored / "test_shutil.py":
        raise RuntimeError("the selected shutil tests must come from the pinned vendored file")
    report = {"cpython_commit": verify_upstream(), "host": "upstream unittest and filesystem setup",
              "native": "shutil.which", "tests": {}}
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        source, program = work / "driver.py", work / "driver"
        source.write_text(driver_source())
        subprocess.run([args.imre.resolve(), "build", source, "-o", program, "--no-cache", "--opt-level", "3",
                        "--clang", args.clang.resolve(), "--runtime", args.runtime.resolve()], check=True, timeout=120)
        if sys.platform == "linux":
            sys.path.insert(0, str(HERE / "full/shutil"))
            from which_defaults import paired
            defaults = paired(args, suite, work)
        else:
            defaults = {}
        for cls in (suite.TestWhich, suite.TestWhichBytes):
            for name in unittest.defaultTestLoader.getTestCaseNames(cls):
                case = cls(name)
                if name == "test_environ_path_missing" and defaults:
                    report["tests"][case.id()] = {"status": "mixed", "native_calls": 2,
                        "optimization_levels": {level: rows[case.id()] for level, rows in defaults.items()}}
                    print("mixed", case.id(), "two native lookups each at -O0 and -O3")
                    continue
                with mock.patch.object(suite.shutil, "which", side_effect=lambda *a, **kw: invoke(program, *a, **kw)) as calls:
                    result = unittest.TestResult()
                    case.run(result)
                if result.wasSuccessful() and result.skipped:
                    status = "skip"
                elif result.wasSuccessful() and result.testsRun == 1 and calls.call_count:
                    status = "pass"
                else:
                    status = "fail"
                report["tests"][case.id()] = {"status": status, "native_calls": calls.call_count,
                    "diagnostics": [s for _, s in result.errors + result.failures + result.skipped]}
                print(status, case.id(), calls.call_count)
        # Real libc and environment defaults remain independently observable.
        with mock.patch.dict(os.environ):
            os.environ.pop("PATH", None)
            for cmd in ("sh", b"sh"):
                expected = suite.shutil.which(cmd)
                if expected is None or invoke(program, cmd) != expected:
                    raise AssertionError("unset PATH uses the platform default")
        name = b"non-utf8-\xff"
        executable = os.fsencode(work) + b"/" + name
        with open(executable, "wb") as f:
            f.write(b"")
        os.chmod(executable, 0o700)
        if invoke(program, name, mode=os.X_OK, path=os.fsencode(work)) != executable:
            raise AssertionError("non-UTF-8 executable name")
        report["controls"] = ["unset PATH (str)", "unset PATH (bytes)", "non-UTF-8 bytes name"]
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(any(v["status"] == "fail" for v in report["tests"].values()))


if __name__ == "__main__":
    sys.exit(main())
