"""Measure public unittest objects and native discovery without adding upstream IDs."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

PIN = "5607950ef232dad16d75c0cf53101d9649d89115"
FIXTURES = {
    "Lib/test/__init__.py": ("test/__init__.py", "b93054b3ecf3a5af96f4772e7208e7a18b5dd4a4"),
    "Lib/test/dis_module.py": ("test/dis_module.py", "afbf600fdee22bbb87b2b4630c8e2ab2df2761f6"),
    "Lib/test/test_errno.py": ("test/test_errno.py", "e7f185c6b1a1812cbe3db181c41c5a5d43be6984"),
    "Lib/test/test_unittest/namespace_test_pkg/bar/__init__.py": ("bar/__init__.py", "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"),
    "Lib/test/test_unittest/namespace_test_pkg/bar/test_bar.py": ("bar/test_bar.py", "05b184d9eba68596b4c9757a40a2e51fe22c487d"),
}
PROBES = {
    "import:unittest": "import unittest\nprint('imported')\n",
    **{f"import:{name}": f"from unittest import {name}\nprint('imported')\n"
       for name in ("TestCase", "TestResult", "TestLoader", "defaultTestLoader", "TestSuite", "TextTestRunner", "mock")},
    "import:test.dis_module": "import test.dis_module as target\ntarget.f()\nprint('imported')\n",
    "module:store": "import test.dis_module as target\nsaved = target\nprint(saved is target)\nprint(saved.__name__)\n",
    "module:argument": "import test.dis_module as target\ndef accept(value):\n    print(value.__name__)\naccept(target)\n",
}

def driver(module, run):
    return (f"import unittest\nimport {module} as target\n"
            "tests = unittest.TestSuite([unittest.defaultTestLoader.loadTestsFromModule(target)])\n"
            "print(tests.countTestCases())\n" + (
                "result = unittest.TextTestRunner(verbosity=2).run(tests)\n"
                "print(result.testsRun)\nprint(result.wasSuccessful())\n" if run else ""))

for label, name in (("empty", "test.dis_module"), ("upstream", "bar.test_bar"),
                    ("failure", "observable_case"), ("errno", "test.test_errno")):
    PROBES["collect:" + label] = driver(name, False)
    PROBES["run:" + label] = driver(name, True)

# This control makes a false zero count, a skipped body, or a fabricated
# result observable. It is not an additional test in any upstream denominator.
OBSERVABLE = """import unittest
class Subject(unittest.TestCase):
    def setUp(self):
        print("setUp")
    def tearDown(self):
        print("tearDown")
    def test_a_passes(self):
        print("pass body")
        self.assertEqual(1, 1)
    def test_b_fails(self):
        print("failing body")
        self.assertEqual(1, 2)
"""

NESTING = """import unittest
visits: list[int] = []
class Nested(unittest.TestCase):
    def test_inner(self):
        visits.append(1)
        self.fail("inner failure")
    def test_outer(self):
        child = unittest.TestResult()
        unittest.TestSuite([Nested("test_inner")]).run(child)
        self.assertEqual(child.testsRun, 1)
        self.assertEqual(len(child.failures), 1)
class Original(Exception):
    pass
class Child(Original):
    pass
class Reclassifies(unittest.TestCase):
    failureException = Original
    def test_custom(self):
        raise Child("counted as failure")
    def test_builtin(self):
        raise AssertionError("counted as error")
"""
# More sequential invocations than the nesting limit prove unwinding on the
# failure path as well as on a normal return. This control contributes no IDs.
PROBES["run:nesting"] = """import nesting_case
import unittest
result = unittest.TestResult()
for index in range(70):
    nesting_case.Nested("test_outer").run(result)
print(len(nesting_case.visits))
print(result.testsRun)
print(result.wasSuccessful())
"""
PROBES["run:failure-category"] = """import nesting_case
import unittest
result = unittest.TestResult()
nesting_case.Reclassifies("test_custom").run(result)
nesting_case.Reclassifies("test_builtin").run(result)
print(result.testsRun)
print(len(result.failures))
print(len(result.errors))
"""
PROBES["run:override"] = Path(__file__).with_name("suite_run_overrides.py").read_text()

def blob(data):
    return hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()

def run(command, root):
    return subprocess.run(command, cwd=root, capture_output=True, text=True, timeout=180)

def runner_text(value, root):
    value = value.replace(str(root), "<root>")
    return re.sub(r"(Ran [0-9]+ tests? in )[0-9]+\.[0-9]{3}(s\n)", r"\1<elapsed>\2", value)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("cpython-source", "imre", "runtime", "clang", "result"):
        parser.add_argument("--" + flag, type=Path, required=True)
    parser.add_argument("--probe", action="append", choices=PROBES)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5):
        parser.error("run with the pinned, test-enabled CPython 3.14.5")
    names = args.probe or list(PROBES)
    records = []
    reference_outputs = {}
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        for path, (local, sha) in FIXTURES.items():
            data = (args.cpython_source / path).read_bytes()
            if blob(data) != sha:
                raise ValueError(f"{path}: expected pinned blob {sha}")
            target = root / local
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        (root / "observable_case.py").write_text(OBSERVABLE)
        (root / "nesting_case.py").write_text(NESTING)
        for name in names:
            entry = root / "entry.py"
            entry.write_text(PROBES[name])
            # -E makes the original package in this temporary root the same
            # import candidate used by Imre even on hosts with PYTHONSAFEPATH.
            reference = run([sys.executable, "-E", "-B", entry], root)
            if reference.returncode:
                raise RuntimeError(f"CPython control {name} failed: {reference.stderr}")
            reference_outputs[name] = {"stdout": reference.stdout, "stderr": runner_text(reference.stderr, root)}
            for reachable in (False, True):
                for level in (0, 3):
                    binary = root / "program"
                    command = [args.imre.resolve(), "build", entry.name, "--runtime", args.runtime.resolve(),
                               "--clang", args.clang.resolve(), "--opt-level", str(level), "--no-cache", "-o", binary]
                    if reachable:
                        command.append("--reachable-only")
                    built = run(command, root)
                    row = {"capability": name, "reachable_only": reachable, "opt_level": level}
                    if built.returncode:
                        row.update(outcome="compile-blocked", reason=(built.stderr or built.stdout).replace(str(root), "<root>").strip())
                    else:
                        observed = run([binary], root)
                        # A correct count or return status need not imply the same
                        # traceback or runner rendering. Report both comparisons.
                        good = observed.returncode == reference.returncode and observed.stdout == reference.stdout
                        diagnostic = runner_text(observed.stderr, root)
                        row.update(outcome="verified" if good else "runtime-mismatch",
                                   stdout=observed.stdout, stderr=diagnostic,
                                   matches_reference_stderr=diagnostic == reference_outputs[name]["stderr"],
                                   returncode=observed.returncode)
                    records.append(row)
    with args.imre.open("rb") as compiler:
        digest = hashlib.file_digest(compiler, "sha256").hexdigest()
    report = {"cpython_commit": PIN, "compiler_sha256": digest, "reference": reference_outputs,
              "fixtures": {path: sha for path, (_, sha) in FIXTURES.items()}, "records": records}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    for name in names:
        print(name, *(row["outcome"] for row in records if row["capability"] == name))

if __name__ == "__main__":
    main()
