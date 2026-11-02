"""Compare the original TestCopyTree and TestMove classes in CPython and native Linux executables."""

import argparse
import ast
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from textwrap import dedent

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import verify_files, verify_interpreter_tests

CLASSES = ("TestCopyTree", "TestMove")
MODULE = "test.test_shutil"
RUNNER = """\
import sys
import unittest
from test.test_shutil import TestCopyTree, TestMove

classes = {"TestCopyTree": TestCopyTree, "TestMove": TestMove}
suite = unittest.TestSuite()
for original in sys.argv[1:]:
    module, cls, name = original.rsplit(".", 2)
    suite.addTest(classes[cls](name))
result = unittest.TextTestRunner(verbosity=2).run(suite)
if result.testsRun != len(sys.argv) - 1 or not result.wasSuccessful():
    sys.exit(1)
"""


def excerpts(path, names, ranges=()):
    """Copy named top-level definitions and selected statements from a verified CPython blob."""
    source = path.read_text()
    lines = source.splitlines(keepends=True)
    spans = list(ranges)
    for node in ast.parse(source).body:
        if isinstance(node, (ast.FunctionDef, ast.ClassDef)) and node.name in names:
            first = min([node.lineno] + [d.lineno for d in node.decorator_list])
            spans.append((first, node.end_lineno))
        elif isinstance(node, ast.Assign) and any(
            isinstance(target, ast.Name) and target.id in names for target in node.targets
        ):
            spans.append((node.lineno, node.end_lineno))
    return "\n".join(dedent("".join(lines[start - 1:end])) for start, end in sorted(spans))


def project(root):
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    shared = HERE.parent / "shared"
    helpers = json.loads((shared / "UPSTREAM.json").read_text())
    if manifest["commit"] != helpers["commit"]:
        raise ValueError("shutil and its shared test helpers have different CPython revisions")
    files = ("Lib/test/support/__init__.py", "Lib/test/support/os_helper.py")
    helper_files = {name: helpers["files"][name] for name in files}
    verify_files(HERE / "upstream", manifest["files"])
    verify_files(shared / "upstream", helper_files)
    verify_interpreter_tests({"files": {**manifest["files"], **helper_files}})

    test = root / "test"
    support = test / "support"
    support.mkdir(parents=True)
    (test / "__init__.py").touch()
    upstream = HERE / "upstream/Lib/test/test_shutil.py"
    (test / "test_shutil.py").write_text(
        "import errno, functools, os, shutil, stat, sys, tempfile, unittest, unittest.mock\n"
        "from shutil import Error\nfrom test.support import os_helper\n"
        "from test.support.os_helper import TESTFN, FakePath\n"
        + excerpts(upstream, (*CLASSES, "BaseTest", "TESTFN2", "TESTFN_SRC", "TESTFN_DST",
                              "_fake_rename", "mock_rename", "create_file", "read_file"))
    )
    (support / "__init__.py").write_text(
        "import os, stat\n" + excerpts(shared / "upstream" / files[0], ("verbose", "_force_run"))
    )
    (support / "os_helper.py").write_text(
        "import os, stat, unittest\n"
        + excerpts(shared / "upstream" / files[1], (
            "TESTFN_ASCII", "TESTFN", "_can_symlink", "can_symlink", "skip_unless_symlink",
            "_can_chmod", "can_chmod", "_can_dac_override", "can_dac_override",
            "skip_unless_dac_override", "FakePath", "create_empty_file", "unlink", "rmtree",
        ), (
            (68, 112), (152, 155),  # Select the first usable non-ASCII test filename.
            (457, 457), (460, 482),  # POSIX branch defining _unlink and _rmtree.
        ))
    )
    entry = root / "run.py"
    entry.write_text(RUNNER)
    return entry


def run_cases(command, label):
    with tempfile.TemporaryDirectory() as cwd:
        run = subprocess.run(command, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                             text=True)
        leftovers = sorted(path.name for path in Path(cwd).iterdir())
    print(f"\n{label} (exit {run.returncode})", flush=True)
    print(run.stdout, end="", flush=True)
    if run.returncode or leftovers:
        raise RuntimeError(f"{label} exited {run.returncode}, leftover files: {leftovers}")
    results = re.findall(r"^test_\w+ \((test\.test_shutil\.\w+\.test_\w+)\) \.\.\. (ok|skipped .+)$",
                         run.stdout, re.MULTILINE)
    if len(dict(results)) != 42 or len(results) != 42 or not re.search(r"^Ran 42 tests in ", run.stdout, re.M):
        raise ValueError(f"{label} did not report all 42 distinct originals")
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("imre", "runtime", "clang"):
        parser.add_argument("--" + option, type=Path, required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or sys.platform != "linux":
        parser.error("run with test-enabled CPython 3.14.5 on Linux")
    with tempfile.TemporaryDirectory() as work:
        root = Path(work)
        entry = project(root)
        original = run_cases([sys.executable, "-B", "-m", "unittest", "-v",
                              *(f"{MODULE}.{name}" for name in CLASSES)], "intact CPython classes")
        ids = [name for name, _ in original]
        if run_cases([sys.executable, "-B", entry, *ids], "focused CPython classes") != original:
            raise ValueError("projected CPython outcomes differ from the intact reference")
        for level in (0, 3):
            binary = root / f"native-O{level}"
            print(f"Building -O{level}", flush=True)
            subprocess.run([args.imre.resolve(), "build", entry, "--runtime-imports", "--module-root", root,
                            "--runtime", args.runtime.resolve(), "--clang", args.clang.resolve(),
                            "--opt-level", str(level), "-o", binary], cwd=root, check=True)
            if run_cases([binary, *ids], f"native -O{level}") != original:
                raise ValueError(f"native -O{level} outcomes differ from the intact reference")


if __name__ == "__main__":
    main()
