"""Build the unchanged fnmatch suite through both public unittest entry points on Linux."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parents[1]))
from stage_tests import stage
from suite_evidence import build_report, occurrence_keys, roles, write_reports

LOADER = """import sys
import unittest
import test.test_fnmatch as target

suite = unittest.defaultTestLoader.loadTestsFromModule(target)
result = unittest.TextTestRunner(verbosity=2).run(suite)
if not result.wasSuccessful():
    sys.exit(1)
"""


def digest(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def outcome(command, failures, total):
    run = subprocess.run(command, text=True, capture_output=True, timeout=120)
    lines = run.stderr.splitlines()
    summary = f"FAILED (failures={failures})" if failures else "OK"
    if (run.returncode != int(bool(failures)) or run.stdout or lines[-1:] != [summary]
            or not any(line.startswith(f"Ran {total} tests in ") for line in lines)):
        raise AssertionError((command, run.returncode, run.stdout, run.stderr))
    return [line for line in lines if " ... " in line]


def break_inputs(original):
    # On a separate copy, require one ordinary failure and two reported subtests.
    for old, new in [(b"check('AbC', 'abc', False, fnmatchcase)", b"check('AbC', 'abc', True, fnmatchcase)"),
                     (b"('ab*', r'(?s:ab.*)", b"('xy*', r'(?s:ab.*)"),
                     (b"('ab*cd', r'(?s:ab.*cd)", b"('xy*cd', r'(?s:ab.*cd)")]:
        if original.count(old) != 1:
            raise ValueError(f"expected a unique control input: {old!r}")
        original = original.replace(old, new)
    return original


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, required=True, type=Path)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or sys.implementation.name != "cpython" or sys.platform != "linux":
        parser.error("use Linux CPython 3.14.5")
    reference = json.loads((HERE / "reference-linux.json").read_bytes())
    total = reference["denominator"]
    imre, runtime, clang = (path.resolve() for path in (args.imre, args.runtime, args.clang))
    proof = {"compiler_sha256": digest(imre), "runtime_sha256": digest(runtime)}
    reports = {}
    with tempfile.TemporaryDirectory() as tmp:
        for failures in (0, 3):
            work = Path(tmp) / ("failure-control" if failures else "upstream")
            stage(work)
            lib = work / "Lib"
            source = lib / "test/test_fnmatch.py"
            loader = lib / "imre_suite.py"
            loader.write_text(LOADER)
            if failures:
                source.write_bytes(break_inputs(source.read_bytes()))
            for entry in (source, loader):
                expected = outcome([sys.executable, "-E", "-B", entry, "-v"], failures, total)
                if not failures:
                    ids = [re.fullmatch(r"\w+ \(([\w.]+)\) \.\.\. ok", line)[1] for line in expected]
                    if entry == source:
                        ids = [name.replace("__main__.", "test.test_fnmatch.", 1) for name in ids]
                    if sorted(ids) != sorted(reference["reference"]):
                        raise ValueError(f"reference discovery mismatch: {ids}")
                for level in (0, 3):
                    binary = work / f"{entry.stem}-O{level}"
                    subprocess.run([imre, "build", entry, "--module-root", lib, "--no-cache",
                                    "--opt-level", str(level), "--runtime", runtime, "--clang", clang,
                                    "-o", binary], check=True, timeout=180)
                    actual = outcome([binary, "-v"], failures, total)
                    if actual != expected:
                        raise AssertionError((entry.name, level, expected, actual))
                    print(f"{entry.name} O{level}: {total} cases, {failures} failures, matched CPython", flush=True)
                    if entry == loader and not failures:
                        evidence = dict(proof, opt_level=level, executable_sha256=digest(binary),
                                        suite_sha256=digest(source), operation="Native fnmatch operations in the unchanged suite.",
                                        fixture="Native imported TestCase classes, loader, runner and assertions.")
                        observations = {key: {"outcome": "pass", "roles": roles(body="imre", assertions="imre",
                                            operation="imre", fixture="imre"), "evidence": evidence}
                                        for key in occurrence_keys(ids)}
                        discovered = [(name, name.split(".")[-2]) for name in ids]
                        reports[level] = build_report(reference, level, discovered, observations)
    write_reports(args.result, [reports[0], reports[3]], reference)


if __name__ == "__main__":
    main()
