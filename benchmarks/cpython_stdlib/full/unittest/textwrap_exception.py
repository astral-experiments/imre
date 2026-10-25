"""Re-run the textwrap owner's gate, then execute its exact type-error test natively."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from suite_evidence import build_report, roles, validate_report, write_reports
from suite_inventory import verify_files, verify_interpreter_tests
from suite_native import Toolchain, method_bytes


CASE = "test.test_textwrap.DedentTestCase.test_type_error"
TEST = "Lib/test/test_textwrap.py"
BLOB = "aca1f427656bb506c3ad369972e1e3ea2757d7d0"
FIXTURE = b"import unittest\nfrom textwrap import dedent\nclass DedentTestCase(unittest.TestCase):\n"
MAIN = b"\nif __name__ == '__main__':\n    unittest.main()\n"
NEGATIVE = b'''    def test_missing(self):
        with self.assertRaisesRegex(TypeError, "expected str object, not"):
            dedent("valid")

    def test_wrong_message(self):
        with self.assertRaisesRegex(TypeError, "^other message$"):
            dedent(b"")
'''


def observe(tools, root, dest):
    method = method_bytes(root / "upstream" / TEST, "DedentTestCase", "test_type_error")
    for body, failing in ((method, False), (NEGATIVE, True)):
        binary = tools.build(FIXTURE + body + MAIN, dest)
        def run(args):
            return subprocess.run([*args, "-v"], capture_output=True, text=True, timeout=60)
        actual = run([binary])
        oracle = run([sys.executable, "-I", "-B", dest / "case.py"])
        if failing:
            for result in (oracle, actual):
                if (result.returncode != 1 or result.stdout
                        or result.stderr.count(" ... FAIL\n") != 2
                        or "TypeError not raised" not in result.stderr
                        or '"^other message$" does not match' not in result.stderr
                        or "FAILED (failures=2)" not in result.stderr):
                    raise AssertionError(result)
        else:
            def transcript(result):
                return (result.returncode, result.stdout,
                        re.sub(r"(?m)^(Ran 1 test in )\d+\.\d+s$", r"\1<elapsed>s", result.stderr))
            marker = "test_type_error (__main__.DedentTestCase.test_type_error) ... ok"
            if oracle.returncode or marker not in oracle.stderr or transcript(actual) != transcript(oracle):
                raise AssertionError((actual, oracle))
    return {"outcome": "pass", "roles": roles(body="imre", assertions="imre", operation="imre", fixture="imre"),
            "evidence": {"operation": "textwrap.dedent and unittest.TestCase.assertRaisesRegex",
                         "fixture": "Original default TestCase fixture; native unittest.main selected and ran the method",
                         "method_sha256": hashlib.sha256(method).hexdigest(), "opt_level": tools.level}}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for flag in ("textwrap-suite", "imre", "runtime", "clang", "result"):
        p.add_argument("--" + flag, type=Path, required=True)
    p.add_argument("--cpython-source", type=Path)
    p.add_argument("--baseline", type=Path,
                   help="the ordinary textwrap report just generated with this compiler")
    p.add_argument("--reachable-only", action="store_true")
    a = p.parse_args()
    if a.baseline and a.reachable_only:
        p.error("the reachable gate generates its own baseline")
    if a.cpython_source:
        sys.path.insert(0, str(a.cpython_source.resolve() / "Lib"))
    root = a.textwrap_suite.resolve()
    manifest = json.loads((root / "UPSTREAM.json").read_bytes())
    ref = json.loads((root / "reference-linux.json").read_bytes())
    if manifest["files"][TEST] != BLOB or ref["denominator"] != 68:
        raise ValueError("requires the original CPython 3.14.5 textwrap suite")
    verify_files(root / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    reports = []
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        compiler = a.imre.resolve()
        if a.reachable_only:
            # The stock textwrap gate constructs Toolchain without a mode flag.
            compiler = work / "imre-reachable"
            compiler.write_text("#!/bin/sh\nexec " + shlex.quote(str(a.imre.resolve()))
                                + ' "$@" --reachable-only\n')
            compiler.chmod(0o755)
        baseline = a.baseline or work / "baseline.json"
        if not a.baseline:
            command = [sys.executable, "-B", root / "run.py", "--imre", compiler,
                       "--runtime", a.runtime.resolve(), "--clang", a.clang.resolve(),
                       "--result", baseline]
            if a.cpython_source:
                command.extend(["--cpython-source", a.cpython_source.resolve()])
            subprocess.run(command, check=True)
        for prior in json.loads(baseline.read_bytes()):
            validate_report(prior, ref)
            level = prior["opt_level"]
            dest = work / str(level)
            dest.mkdir()
            observed = observe(Toolchain(compiler, a.runtime, a.clang, level), root, dest)
            original = {(row["id"], row["occurrence"]): row for row in prior["records"]}
            observations = {key: {k: row[k] for k in ("outcome", "roles", "reason", "evidence") if k in row}
                            for key, row in original.items()}
            observations[CASE, 1] = observed
            discovered = [(r["id"], r["fixture_variant"]) for r in prior["records"]]
            exceptions = {(*key, level): row["exception"] for key, row in original.items() if "exception" in row}
            report = build_report(ref, level, discovered, observations, exceptions=exceptions)
            for row in report["records"]:
                key = row["id"], row["occurrence"]
                if key == (CASE, 1):
                    if row["classification"] != "fully-native":
                        raise AssertionError(row)
                elif row != original[key]:
                    raise AssertionError(f"unexpected evidence change for {key}")
            reports.append(report)
            print(f"O{level}: {report['counts']}", flush=True)
    write_reports(a.result, reports, ref)
    with a.imre.open("rb") as compiler_file:
        print("compiler SHA-256:", hashlib.file_digest(compiler_file, "sha256").hexdigest())


if __name__ == "__main__":
    main()
