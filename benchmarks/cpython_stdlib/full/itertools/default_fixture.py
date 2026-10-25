"""Try each original itertools method with the native default unittest fixture."""

import argparse
import doctest
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
SUPPORT = HERE.parents[1]
sys.path[:0] = [str(HERE), str(SUPPORT), str(HERE.parent / "unittest")]
from projection_audit import stage_test_tree
from suite_evidence import ROLES, build_report, occurrence_keys, validate_pair, write_reports
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import NativeDefaultMethod, Toolchain

REFERENCE_ENTRY = """import runpy, sys
root, entry = sys.argv[1:3]
sys.path.insert(0, root)
sys.argv[:] = [entry, *sys.argv[3:]]
runpy.run_path(entry, run_name='__main__')
"""


def attempt(case, tools, root, source, blob, prior):
    with tempfile.TemporaryDirectory(dir=root, prefix="probe_") as temp:
        work = Path(temp)
        (work / "__init__.py").write_bytes(b"")
        (work / "test").symlink_to(root / "test", target_is_directory=True)
        observation = NativeDefaultMethod(tools, work, source, blob,
                                          ("unknown", "awaiting operation review"), assertions="unknown").run(case)
        if observation["outcome"] == "pass":
            proof = observation["evidence"]
            known = prior.get("evidence", {})
            if (prior["outcome"] != "pass" or known.get("method_sha256") != proof["method_sha256"]
                    or not known.get("operation")
                    or any(prior["roles"][role] != "imre" for role in ("body", "assertions", "operation"))):
                raise ValueError(f"a new native fixture pass needs a review of its original operation: {case.id()}")
            # Re-run the same staged class and default fixture under the reference interpreter.
            oracle = subprocess.run([sys.executable, "-I", "-B", "-c", REFERENCE_ENTRY, root, work / "case.py", "-v"], cwd=work,
                                    capture_output=True, text=True, timeout=60)
            stderr = re.sub(r"(?m)^(Ran 1 test in )\d+\.\d+s$", r"\1<elapsed>s", oracle.stderr)
            if (oracle.returncode, oracle.stdout, stderr) != (proof["status"], proof["stdout"], proof["stderr"]):
                raise ValueError(f"staged native and CPython fixture transcripts differ: {case.id()}")
            proof["staged_reference_matches"] = True
            proof["operation"] = known["operation"]
            observation["roles"].update(operation="imre", assertions="imre")
        if "reason" in observation:
            reason = observation["reason"]
            for path, label in ((work, "<case>"), (root, "<root>")):
                reason = reason.replace(str(path), label).replace(str(path).lstrip("/"), label)
            observation["reason"] = reason.replace(work.name, "<case>")
        return observation


def run(args, root, manifest, reference, previous):
    source = "Lib/test/test_itertools.py"
    reports = []
    for prior_report in previous:
        level = prior_report["opt_level"]
        tools = Toolchain(args.imre, args.runtime, args.clang, level, args.reachable_only, root)
        prior = {(r["id"], r["occurrence"]): r for r in prior_report["records"]}
        discovered = list(cases(load_suite(manifest["module"])))
        keys = list(occurrence_keys(c.id() for c in discovered))
        if set(keys) != prior.keys():
            raise ValueError("original discovery differs from the validated primary denominator")
        variants, observations, exceptions = [], {}, {}
        for key, case in zip(keys, discovered, strict=True):
            old = prior[key]
            if old["classification"] in {"reviewed-exclusion", "reference-skip"}:
                variant = old["fixture_variant"]
                obs = {"outcome": "not-run", "roles": dict.fromkeys(ROLES, "unobserved")}
                exceptions[(*key, level)] = old["exception"]
                if old["classification"] == "reviewed-exclusion":
                    obs.update(outcome="excluded", reason=old["reason"])
            elif isinstance(case, doctest.DocTestCase):
                variant = "original DocTestCase (outside native default TestCase fixture)"
                obs = {"outcome": "blocked", "roles": dict.fromkeys(ROLES, "unobserved"),
                       "reason": "The original DocTestCase uses its own fixture and example namespace. See the primary report for its actual execution.",
                       "evidence": {"stage": "original fixture"}}
            else:
                variant = "original unittest.TestCase default fixture in a native executable"
                obs = attempt(case, tools, root, HERE / "upstream" / source, manifest["files"][source], old)
            variants.append((case.id(), variant))
            observations[key] = obs
        report = build_report(reference, level, variants, observations, exceptions=exceptions)
        reports.append(report)
        print(f"O{level}: {report['denominator']} occurrences: {report['counts']}", flush=True)
    write_reports(args.result, reports, reference)
    return any(r["classification"] in {"failed", "unexpected-skip", "reference-disagreement"}
               for report in reports for r in report["records"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang", "cpython-source", "result"):
        parser.add_argument("--" + arg, required=True, type=Path)
    parser.add_argument("--reachable-only", action="store_true")
    args = parser.parse_args()
    if (sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5)
            or not sys._is_gil_enabled() or sys.flags.optimize):
        parser.error("primary discovery requires a regular CPython 3.14.5 build")
    sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    manifest = json.loads((HERE / "UPSTREAM.json").read_bytes())
    reference = json.loads((HERE / "reference-linux.json").read_bytes())
    previous = json.loads((HERE / "evidence-linux.json").read_bytes())
    if (manifest["module"], manifest["commit"]) != (reference["module"], reference["cpython_commit"]):
        raise ValueError("suite and reference disagree")
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    validate_pair(previous, reference)
    from test import support
    support.use_resources = ["cpu"]  # Attempt the long-chain method under the primary report's configuration.
    with tempfile.TemporaryDirectory(prefix="imre-itertools-default-") as tmp:
        root = Path(tmp).resolve()
        receipt = json.loads((HERE.parent / "unittest/cpython-test-tree.json").read_bytes())
        count = stage_test_tree(args.cpython_source, root, receipt)
        print(f"Verified {count} original fixture blobs in {receipt['sha']}", flush=True)
        return run(args, root, manifest, reference, previous)


if __name__ == "__main__":
    sys.exit(main())
