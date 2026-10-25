"""Run the pinned ZIP constructor error cases against an Imre child on POSIX."""
import argparse
import ast
from contextlib import chdir
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
import warnings
import zipfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from upstream import verify_upstream

CASES = {"test_non_existent_file_raises_OSError": 1, "test_empty_file_raises_BadZipFile": 2,
         "test_close_erroneous_file": 1, "test_open_empty_file": 1}
DRIVER = '''from pathlib import Path
import sys
from zipfile import BadZipFile, ZipFile

try:
    with ZipFile(file=Path(sys.argv[1]), mode="r") as archive:
        names = archive.namelist()
        names.append("only the returned list changes")
    archive.close()
    with archive as alias:
        print("ok")
        print(repr(alias.namelist()))
except BadZipFile as error:
    print("BadZipFile")
    print(str(error))
except FileNotFoundError as error:
    print("FileNotFoundError")
    print(str(error))
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    parser.add_argument("--opt-level", choices=("0", "3"), default="3")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or os.name != "posix":
        parser.error("the ZIP reference requires CPython 3.14.5 on POSIX")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    upstream = HERE / "upstream/Lib/test"
    test.__path__.insert(0, str(upstream))
    from test.test_zipfile import test_core as suite
    if Path(suite.__file__).resolve() != upstream / "test_zipfile/test_core.py":
        raise RuntimeError("test_core must come from the verified vendored file")
    report = {"cpython_commit": verify_upstream(), "opt_level": args.opt_level,
              "host": "original test bodies, assertions and file setup",
              "native": "ZipFile constructor, close and namelist", "tests": {}}
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        driver, executable = work / "probe.py", work / "probe"
        driver.write_text(DRIVER)
        subprocess.run([args.imre.resolve(), "build", driver, "--opt-level", args.opt_level, "--no-cache",
                        "--runtime", args.runtime.resolve(), "--clang", args.clang.resolve(), "-o", executable],
                       check=True, timeout=120)
        def invoke(path):
            run = subprocess.run([executable, path], check=True, text=True, capture_output=True, timeout=30)
            if run.stderr:
                raise AssertionError(run.stderr)
            tag, detail = run.stdout.removesuffix("\n").split("\n", 1)
            return tag, detail

        # Construct before patching: CPython supplies controlled archives, never the operation under test.
        archive = work / "names.zip"
        names = ["d/", "d/alpha", "same", "same", "café", "雪"]
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", UserWarning)
            with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as zf:
                for i, name in enumerate(names):
                    zf.writestr(name, bytes([i]) * 8192)
                zf.comment = b"ordinary archive comment"
        for prefix in (b"", b"self-extracting prefix\n"):
            file = work / "prefix.zip"
            file.write_bytes(prefix + archive.read_bytes())
            tag, detail = invoke(file)
            with zipfile.ZipFile(file) as reference:
                assert tag == "ok" and ast.literal_eval(detail) == reference.namelist() == names
        report["ordered_name_controls"] = 2
        with chdir(work):
            for name, count in CASES.items():
                calls = []
                def native_open(path, mode="r"):
                    assert mode == "r"
                    tag, detail = invoke(path)
                    calls.append(tag)
                    if tag == "BadZipFile":
                        raise zipfile.BadZipFile(detail)
                    if tag == "FileNotFoundError":
                        raise FileNotFoundError(detail)
                    raise AssertionError(f"expected an unsuccessful open, got {tag}: {detail}")
                case = suite.OtherTests(name)
                result = unittest.TestResult()
                with mock.patch.object(suite.zipfile, "ZipFile", native_open):
                    case.run(result)
                passed = result.wasSuccessful() and result.testsRun == 1 and not result.skipped and len(calls) == count
                report["tests"][case.id()] = {"status": "pass" if passed else "fail", "native_open_results": calls,
                                              "diagnostics": [r for _, r in result.errors + result.failures + result.skipped]}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    for case, row in report["tests"].items():
        print(row["status"], case, *row["native_open_results"])
        for detail in row["diagnostics"]:
            print(detail, file=sys.stderr)
    return int(any(row["status"] != "pass" for row in report["tests"].values()))


if __name__ == "__main__":
    sys.exit(main())
