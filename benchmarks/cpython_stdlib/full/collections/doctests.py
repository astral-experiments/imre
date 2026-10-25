"""Compile the complete pinned collections doctests; check output per prompt."""

import argparse
import ast
import collections
import doctest
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap

HERE = Path(__file__).resolve().parent
MARK = "___imre_doctest_prompt___"


def program(test):
    # Each DocTest gets its own namespace, shared by all its prompts. Display
    # every non-None expression result, irrespective of the expected output.
    source = []
    if test.name.startswith("collections."):
        source.append(f"from collections import {test.name.split('.')[1]}\n")
    for i, example in enumerate(test.examples):
        src = example.source
        if isinstance(ast.parse(src, mode="single").body[0], ast.Expr):
            label = f"_imre_display_{i}"
            src = f"{label} = (\n{src})\nif {label} is not None:\n    print(repr({label}))\n"
        if example.exc_msg:
            src = ("try:\n" + textwrap.indent(src, "    ")
                   + "except Exception as _imre_exc:\n"
                   + "    print(type(_imre_exc).__name__ + ': ' + str(_imre_exc))\n")
        source.extend([src, f"print({MARK!r})\n"])
    return "".join(source)


def mismatch(test, output):
    chunks = output.split(MARK + "\n")
    if len(chunks) != len(test.examples) + 1 or chunks[-1]:
        return f"incomplete prompt output: {output!r}"
    checker = doctest.OutputChecker()
    for i, (example, got) in enumerate(zip(test.examples, chunks)):
        want = example.exc_msg if example.exc_msg is not None else example.want
        flags = sum(flag for flag, enabled in example.options.items() if enabled)
        if not checker.check_output(want, got, flags):
            return f"prompt {i + 1}: expected {want!r}, got {got!r}"
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--opt-level", type=int, choices=(0, 3), required=True)
    parser.add_argument("--compiler-revision")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use the pinned test-enabled CPython 3.14.5")
    from test import test_deque
    sys.path.insert(0, str(HERE.parents[1]))
    from suite_inventory import verify_files

    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_files(Path(collections.__file__).resolve().parents[2], {
        name: manifest["files"][name] for name in ("Lib/collections/__init__.py", "Lib/test/test_deque.py")
    })
    tests = [test for mod in (collections, test_deque)
             for test in doctest.DocTestFinder().find(mod) if test.examples]
    outcomes = {}
    with tempfile.TemporaryDirectory() as tmp:
        for test in tests:
            wd = Path(tmp) / test.name
            wd.mkdir()
            path = wd / "case.py"
            path.write_text(program(test))
            def run(command):
                return subprocess.run(command, cwd=wd, text=True, capture_output=True, timeout=120)
            # Validate the display adapter against the pinned expectations
            # first. These host executions are never reported as native.
            control = run([sys.executable, "-B", path])
            error = control.stderr or mismatch(test, control.stdout)
            if control.returncode or error:
                raise ValueError(f"{test.name}: exit {control.returncode}: {error}")
            binary = wd / "case"
            build = run([args.imre.resolve(), "build", path, "-o", binary, "--no-cache", "--quiet",
                         "--opt-level", str(args.opt_level), "--runtime", args.runtime.resolve(),
                         "--clang", args.clang.resolve()])
            diagnostic = None
            if build.returncode:
                status = "compile-blocked" if build.returncode == 1 and ": Imre:" in build.stderr else "build-failed"
                diagnostic = build.stderr.replace(str(wd) + "/", "")
            else:
                native = run([binary])
                diagnostic = native.stderr or mismatch(test, native.stdout)
                status = "failed" if native.returncode or diagnostic else "native-body/host-output-check"
            outcomes[test.name] = {"examples": len(test.examples), "status": status}
            if diagnostic:
                outcomes[test.name]["diagnostic"] = diagnostic.rstrip()
            print(status, test.name, flush=True)
    report = {"cpython_commit": manifest["commit"], "opt_level": args.opt_level, "tests": outcomes}
    if args.compiler_revision:
        report["compiler_revision"] = args.compiler_revision
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(any(row["status"] != "native-body/host-output-check" for row in outcomes.values()))


if __name__ == "__main__":
    sys.exit(main())
