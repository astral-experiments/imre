"""Probe every reference-passing pathlib occurrence with native unittest default fixtures."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import sys
import tempfile

HERE = Path(__file__).resolve().parent
BENCH = HERE.parents[1]
sys.path[:0] = [str(HERE), str(BENCH)]
from evidence import exceptions
from suite_evidence import occurrence_keys, reference_cases, write_reports
from suite_inventory import cases, load_suite, verify_interpreter_tests
from suite_native import NativeDefaultMethod, Toolchain, run_evidence
from upstream import verify_upstream

OPERATIONS = {
    "test.test_pathlib.test_pathlib.UnsupportedOperationTest.test_is_notimplemented":
        "pathlib.UnsupportedOperation inheritance and instance construction",
}


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def main():
    import test
    from test import support

    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--imre-commit", required=True)
    args = parser.parse_args()
    if (sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5)
            or not sys.flags.warn_default_encoding or not __debug__ or sys.platform != "linux"
            or not re.fullmatch(r"[0-9a-f]{40}", args.imre_commit)):
        parser.error("requires Linux CPython 3.14.5 with reference flags and a full Imre commit")

    reference = json.loads((HERE / "reference-linux.json").read_bytes())
    if verify_upstream() != reference["cpython_commit"]:
        raise ValueError("reference and test blobs differ")
    manifest = json.loads((BENCH / "UPSTREAM.json").read_bytes())
    files = {name: blob for name, blob in manifest["files"].items()
             if name.startswith("Lib/test/test_pathlib/")}
    verify_interpreter_tests({"files": files})
    support.use_resources = ["all"]
    refs = reference_cases(reference)
    test_root = Path(test.__file__).resolve().parent
    source = {"imre_commit": args.imre_commit, "compiler_sha256": digest(args.imre),
              "runtime_sha256": digest(args.runtime), "runner_sha256": digest(BENCH / "suite_native.py")}
    reports = []
    with tempfile.TemporaryDirectory(prefix="imre-pathlib-default-") as work:
        for level in (0, 3):
            tool = Toolchain(args.imre, args.runtime, args.clang, level)
            suite = load_suite(reference["module"])
            leaves = list(cases(suite))
            entries = {}
            for case, key in zip(leaves, occurrence_keys(c.id() for c in leaves), strict=True):
                if refs[key] != "pass":
                    continue
                path = Path(sys.modules[type(case).__module__].__file__).resolve()
                name = "Lib/test/" + path.relative_to(test_root).as_posix()
                op = (("imre", OPERATIONS[key[0]]) if key[0] in OPERATIONS else
                      ("none", "requires operation review if this method runs"))
                entries[key] = NativeDefaultMethod(tool, Path(work), BENCH / "upstream" / name, files[name], op)
            report = run_evidence(suite, {}, reference, level, native_defaults=entries,
                                  exceptions=exceptions(reference, level))
            for row in report["records"]:
                if row["outcome"] == "pass" and row["id"] not in OPERATIONS:
                    raise ValueError("review the operation before recording: " + row["id"])
            report["default_fixture_probe"] = source
            reports.append(report)
            print(f"O{level}: {report['counts']}")
    write_reports(args.result, reports, reference)


if __name__ == "__main__":
    main()
