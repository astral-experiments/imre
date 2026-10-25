"""Compare native doctest discovery and execution with CPython 3.14.5."""

import argparse
import doctest
import importlib
from itertools import batched
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import verify_files
from suite_native import NativeCompileError, Toolchain


def compare(toolchain, directory, source):
    binary = toolchain.build(source, directory)
    reference = subprocess.run(
        [sys.executable, "-B", directory / "case.py"], cwd=directory, capture_output=True,
        env={**os.environ, "PYTHONPATH": str(directory)}, check=True, timeout=60,
    )
    native = subprocess.run([binary], cwd=directory, capture_output=True, timeout=60)
    if (native.returncode, native.stdout, native.stderr) != (0, reference.stdout, reference.stderr):
        raise AssertionError(f"{directory.name} at O{toolchain.level}: {native!r}\nreference: {reference!r}")
    return native.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--require-module", action="append", default=[])
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("run with the pinned CPython 3.14.5")
    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    verify_files(HERE / "upstream", manifest["files"])
    reports = []
    with tempfile.TemporaryDirectory(prefix="imre-doctest-") as temporary:
        for level in (0, 3):
            tools = Toolchain(args.imre, args.runtime, args.clang, level)
            root = Path(temporary) / str(level)
            root.mkdir()
            for control in ("discovery", "execute"):
                work = root / control
                work.mkdir()
                for fixture in ("examples.py", "mismatch.py"):
                    shutil.copyfile(HERE / "controls" / fixture, work / fixture)
                compare(tools, work, (HERE / "controls" / (control + ".py")).read_bytes())
            work = root / "skip"
            work.mkdir()
            sample = HERE / "upstream/Lib/test/test_doctest/sample_doctest_skip.py"
            shutil.copyfile(sample, work / sample.name)
            count, *lines = compare(tools, work, (HERE / "controls/skip.py").read_bytes()).decode().splitlines()
            skip_cases = [{"id": name, "examples": int(size), "runTest": normal, "debug": debug}
                          for name, size, normal, debug in batched(lines, 4, strict=True)]
            if not (int(count) == len(skip_cases) == 6 and sum(c["examples"] for c in skip_cases) == 9):
                raise AssertionError(f"unexpected original skip sample discovery: {skip_cases}")
            print(f"O{level}: original skip sample matches (6 cases, 9 examples)")
            modules = {}
            for name in ("itertools", "json", "json.encoder", "json.decoder"):
                work = root / name
                work.mkdir()
                cases = list(doctest.DocTestSuite(importlib.import_module(name)))
                row = modules[name] = {
                    "ids": [case.id() for case in cases],
                    "examples": sum(len(case._dt_test.examples) for case in cases),
                }
                source = (f"import doctest\nimport {name}\nsuite = doctest.DocTestSuite({name})\n"
                          "print(suite.countTestCases())\nfor case in suite:\n    print(case.id())\nsuite.debug()\n")
                try:
                    compare(tools, work, source.encode())
                except NativeCompileError as exc:
                    if not cases or name in args.require_module:
                        raise
                    row.update(outcome="blocked", diagnostic=str(exc).strip().replace(str(work), "<entry>"))
                else:
                    row["outcome"] = "pass" if cases else "empty-suite"
                print(f"O{level} {name}: {row['outcome']} ({row['examples']} examples)")
            reports.append({"opt_level": level, "controls": ["discovery", "execute", "skip"],
                            "skip_sample": skip_cases, "modules": modules})
    args.result.write_text(json.dumps(reports, indent=2) + "\n")


if __name__ == "__main__":
    main()
