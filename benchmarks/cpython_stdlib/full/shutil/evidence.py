"""Run the available unchanged shutil routines and retain the complete release denominator."""
import argparse
import ast
from contextlib import chdir
import json
from pathlib import Path
import sys
from tempfile import TemporaryDirectory

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path[:0] = [str(HERE), str(STDLIB)]
import copyfile
import native_copy
import native_rmtree
import tree_ops
import move_ops
import which_cases
from suite_evidence import roles, write_reports
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, run_evidence

SOURCE = STDLIB / "upstream/Lib/test/test_shutil.py"
# These three decorators evaluate a feature probe in test.support, which supplies
# the skip reason. The other decorators contain their reason directly in the source.
PROBES = {
    "requires zstd": ("support.requires_zstd()", "compression.zstd failed the CPython feature probe"),
    "no non-broken extended attribute support": ("os_helper.skip_unless_xattr", "not os_helper.can_xattr() on the reference filesystem"),
    "requires CAP_DAC_OVERRIDE": ("os_helper.skip_unless_dac_override", "not os_helper.can_dac_override() for the reference process"),
}


def reference_skips(suite, reference, level):
    classes = {node.name: node for node in ast.parse(SOURCE.read_bytes()).body if isinstance(node, ast.ClassDef)}
    result = {}
    for case in cases(suite):
        outcome = reference["reference"][case.id()]
        if not isinstance(outcome, dict) or "skipped" not in outcome:
            continue
        reason = outcome["skipped"]
        variant = f"{type(case).__module__}.{type(case).__qualname__}"
        matches = []
        for base in type(case).__mro__:
            if base.__module__ != reference["module"]:
                continue
            cls = classes[base.__name__]
            methods = [m for m in cls.body if isinstance(m, ast.FunctionDef) and m.name == case._testMethodName]
            for node in [cls, *methods]:
                for dec in node.decorator_list:
                    expr = ast.unparse(dec)
                    if reason in PROBES and expr == PROBES[reason][0]:
                        condition = PROBES[reason][1]
                    elif (isinstance(dec, ast.Call) and isinstance(dec.func, ast.Attribute)
                          and dec.func.attr in {"skipIf", "skipUnless"} and len(dec.args) == 2
                          and isinstance(dec.args[1], ast.Constant) and dec.args[1].value == reason):
                        condition = ast.unparse(dec.args[0])
                        if dec.func.attr == "skipUnless":
                            condition = f"not ({condition})"
                    else:
                        continue
                    matches.append((condition, dec.lineno))
        if len(matches) != 1:
            raise ValueError(f"{case.id()}: cannot identify the original condition for {reason!r}: {matches}")
        condition, lineno = matches[0]
        # This suite has no repeated ID; fail if future discovery changes this.
        key = case.id(), 1, level
        if key in result:
            raise ValueError(f"duplicate reference occurrence: {key}")
        result[key] = {"kind": "upstream-skip", "condition": condition, "reason": reason,
                       "source": f"Lib/test/test_shutil.py:{lineno}", "fixture_variant": variant,
                       "platform": reference["platform"], "configuration": "CPython 3.14.5 Linux reference; " + variant}
    return result


def run_level(args, module, reference, level, directory):
    from attempt_methods import proof_for
    tools = Toolchain(args.imre, args.runtime, args.clang, level)
    executions, calls = which_cases.executors(tools, directory, SOURCE, module)
    executions.update(copyfile.executors(args, module, directory, level, with_roles=True))
    executions.update(native_copy.executors(args, module, directory, level, with_roles=True))
    executions.update(native_rmtree.executors(args, module, directory, level, with_roles=True))
    tree_work = directory / "tree"
    tree_work.mkdir()
    executions.update(tree_ops.executors(module, tools, tree_work, with_roles=True))
    move_work = directory / "move"
    move_work.mkdir()
    executions.update(move_ops.executors(module, tools, move_work, with_roles=True))
    preflight = {}
    for cls, operation, blockers in (
        (module.TestCopy, "shutil metadata", native_copy.MOCK_CASES),
        (module.TestRmTree, "shutil.rmtree", native_rmtree.BLOCKERS),
        (module.TestCopyTree, "shutil.copytree", tree_ops.BLOCKERS),
    ):
        for name, reason in blockers.items():
            test_id = cls(name).id()
            if reference["reference"][test_id] == "pass":
                preflight[test_id, 1] = {"outcome": "blocked", "reason": reason,
                                         "roles": roles(body="unobserved", assertions="unobserved",
                                                        operation="unobserved", fixture="unobserved"),
                                         "evidence": {"operation": operation, "fixture": reason}}
    attempts = json.loads(args.method_attempts.read_text())
    if (attempts["schema"] != "imre.shutil-method-attempts.v1"
            or attempts["cpython_commit"] != reference["cpython_commit"]
            or set(attempts["cases"]) != set(reference["reference"])):
        raise ValueError("complete-method probes do not cover the pinned denominator")
    proof = {}
    for name, ref in reference["reference"].items():
        if ref != "pass":
            continue
        record = proof_for(attempts, name, level)
        stage = record["status"]
        if stage not in {"compile-blocked", "body-completed", "failed", "fixture-error", "fixture-skip"}:
            raise ValueError(f"{name}: incomplete method attempt: {record['status']}")
        proof[name] = {"artifact": args.method_attempts.name, "stage": stage}
        if "source" in record:
            proof[name].update(source=record["source"], method_sha256=record["method_sha256"])
        if stage in {"failed", "fixture-error", "fixture-skip"}:
            # A working adapter must not hide a real failure in the complete routine.
            executions.pop(name, None)
            preflight[name, 1] = {
                "outcome": {"failed": "failure", "fixture-error": "error", "fixture-skip": "skip"}[stage],
                "reason": record["reason"],
                "roles": roles(body="imre" if stage == "failed" else "unobserved",
                               assertions="unobserved", operation="unobserved", fixture="cpython"),
                "evidence": {"stage": stage, "fixture": attempts["fixture_execution"]}}
            continue
        if name not in executions and (name, 1) not in preflight:
            if record["status"] != "compile-blocked":
                raise ValueError(f"{name}: review the successful compile before classifying it")
            preflight[name, 1] = {
                "outcome": "blocked", "reason": record["reason"],
                "roles": roles(body="unobserved", assertions="unobserved", operation="unobserved", fixture="cpython"),
                "evidence": {"stage": "compile", "fixture": attempts["fixture_execution"],
                             "detail": "complete unchanged method could not be compiled; operation unobserved"}}
    suite = load_suite(reference["module"])
    report = run_evidence(suite, executions, reference, level, preflight=preflight,
                          exceptions=reference_skips(suite, reference, level))
    for row in report["records"]:
        if row["id"] in proof:
            row["evidence"]["complete_method_attempt"] = proof[row["id"]]
        if row["outcome"] == "failure" and calls.get(row["id"]):
            # A failed assertion prevents the adapter from returning its normal
            # observation. Its counter still proves the native lookup occurred.
            missing = row["id"].endswith(".test_environ_path_missing")
            row["roles"].update(body="cpython", assertions="cpython",
                                operation="mixed" if missing else "imre", fixture="cpython")
            row["evidence"].update(operation="shutil.which", native_calls=calls[row["id"]], opt_level=level,
                                   fixture="original TestWhich environment and os.confstr/os.defpath patches")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("imre", "runtime", "clang"):
        parser.add_argument("--" + arg, required=True, type=lambda v: Path(v).resolve())
    parser.add_argument("--result", type=Path, default=HERE / "evidence-linux.json")
    parser.add_argument("--method-attempts", type=lambda p: Path(p).resolve(),
                        default=HERE / "method-attempts-linux.json")
    parser.add_argument("--cpython-source", type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or sys.platform != "linux":
        parser.error("requires the test-enabled CPython 3.14.5 Linux reference")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    reference = json.loads((HERE / "reference-linux.json").read_text())
    if reference["cpython_commit"] != manifest["commit"]:
        raise ValueError("reference commit changed")
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    import test
    test.__path__.insert(0, str(SOURCE.parent))
    reports = []
    for level in (0, 3):
        with TemporaryDirectory() as tmp, chdir(tmp):
            from test import test_shutil
            if Path(test_shutil.__file__).resolve() != SOURCE:
                raise ValueError("loaded a different test_shutil")
            reports.append(run_level(args, test_shutil, reference, level, Path(tmp)))
    write_reports(args.result, reports, reference)
    for r in reports:
        print(f"-O{r['opt_level']}: {r['denominator']} occurrences, {r['counts']}")
    return int(any(r["counts"].get(k, 0) for r in reports for k in ("failed", "unexpected-skip")))


if __name__ == "__main__":
    sys.exit(main())
