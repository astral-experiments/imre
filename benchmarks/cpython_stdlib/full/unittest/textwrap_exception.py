"""Re-run the textwrap owner's gate, then execute its exact type-error test natively."""
import argparse
import hashlib
import json
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from suite_evidence import build_report, validate_report, write_reports
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, native_fixture_method, run_evidence


CASE = "test.test_textwrap.DedentTestCase.test_type_error"
TEST = "Lib/test/test_textwrap.py"
BLOB = "aca1f427656bb506c3ad369972e1e3ea2757d7d0"


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for flag in ("textwrap-suite", "imre", "runtime", "clang", "result"):
        p.add_argument("--" + flag, type=Path, required=True)
    p.add_argument("--reachable-only", action="store_true")
    a = p.parse_args()
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
        baseline = work / "baseline.json"
        subprocess.run([sys.executable, "-B", root / "run.py", "--imre", compiler,
                        "--runtime", a.runtime.resolve(), "--clang", a.clang.resolve(),
                        "--result", baseline], check=True)
        for prior in json.loads(baseline.read_bytes()):
            validate_report(prior, ref)
            level = prior["opt_level"]
            dest = work / str(level)
            dest.mkdir()
            tools = Toolchain(a.imre, a.runtime, a.clang, level, a.reachable_only)
            case = native_fixture_method(tools, dest, root / "upstream" / TEST,
                                        "DedentTestCase", CASE, imports="from textwrap import dedent",
                                        operation="textwrap.dedent and unittest.TestCase.assertRaisesRegex")
            targeted = run_evidence(load_suite(ref["module"]), {}, ref, level,
                                    native_fixtures={(CASE, 1): case})
            original = {(row["id"], row["occurrence"]): row for row in prior["records"]}
            observations = {}
            for row in targeted["records"]:
                key = row["id"], row["occurrence"]
                source = row if key == (CASE, 1) else original[key]
                observations[key] = {k: source[k] for k in ("outcome", "roles", "reason", "evidence")
                                     if k in source}
            discovered = [(r["id"], r["fixture_variant"]) for r in targeted["records"]]
            report = build_report(ref, level, discovered, observations)
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
