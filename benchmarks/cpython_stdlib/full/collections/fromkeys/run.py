"""Run the unchanged dict.fromkeys method separately from the 659 collections IDs."""

import argparse
import hashlib
import json
from pathlib import Path
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2]))
from suite_inventory import load_suite
from suite_evidence import build_report, write_reports
from suite_native import Toolchain, method_bytes, native_method, run_in_fixtures
from reference import reference


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--compiler-revision", required=True)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use the pinned test-enabled CPython 3.14.5")
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    ref = reference()
    if ref != json.loads((HERE / f"reference-{sys.platform}.json").read_bytes()):
        parser.error("discovery or pinned dependency reference changed")
    case_id = manifest["selected_id"]
    path = HERE / "upstream/Lib/test/test_dict.py"
    body = method_bytes(path, "DictTest", "test_fromkeys")
    report = {"cpython_commit": manifest["commit"], "compiler_revision": args.compiler_revision,
              "reference": "pass", "discovered_in_test_dict": len(ref["discovered_in_enclosing_module"]), "selected_denominator": 1,
              "id": case_id, "method_sha256": hashlib.sha256(body).hexdigest(), "levels": {}}
    evidence = []
    for level in (0, 3):
        toolchain = Toolchain(args.imre, args.runtime, args.clang, level)
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            # The referenced global is deliberately present even if UserDict is
            # not yet native. Keep all nested classes and assertions intact.
            fixture = "import collections\nimport unittest\n\nclass NativeCase(unittest.TestCase):\n"
            try:
                execute = native_method(toolchain, directory, path, "DictTest", case_id, fixture,
                                        operation=("imre", "dict.fromkeys including subclass construction and iteration"))
            except (ValueError, RuntimeError) as exc:
                report["levels"][level] = {"outcome": "blocked", "diagnostic": str(exc).replace(temp, "<work>"),
                                           "roles": {"body": "unobserved", "assertions": "unobserved",
                                                     "operation": "unobserved", "fixture": "unobserved"}}
            else:
                ran, failures, skips = run_in_fixtures(load_suite(manifest["module"]), {case_id: execute})
                report["levels"][level] = {"outcome": "failed" if failures or skips else "native-body-host-fixture",
                                           "executions": ran, "failures": failures, "skips": skips}
            row = report["levels"][level]
            if row["outcome"] == "native-body-host-fixture":
                observation = row["executions"][case_id][0]["observation"]
                observation = {**observation, "outcome": "pass"}
            else:
                observation = {"outcome": row["outcome"], "reason": row.get("diagnostic") or str(row.get("failures")),
                               "roles": {k: "unobserved" for k in ("body", "assertions", "operation", "fixture")},
                               "evidence": {}}
            observation["evidence"] = {**observation["evidence"], "compiler_revision": args.compiler_revision,
                                       "method_sha256": report["method_sha256"]}
            evidence.append(build_report(ref, level, [(case_id, "test.test_dict.DictTest")],
                                         {(case_id, 1): observation}))
    write_reports(args.result.with_name(f"evidence-fromkeys-{sys.platform}.json"), evidence, ref)
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    return int(any(row["outcome"] == "failed" for row in report["levels"].values()))


if __name__ == "__main__":
    sys.exit(main())
