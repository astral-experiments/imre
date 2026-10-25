"""Run all four companion methods and their original worker routines on a free-threaded oracle."""

import argparse
import hashlib
import json
from pathlib import Path
import sys
import sysconfig
from tempfile import TemporaryDirectory

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import build_report, occurrence_keys, roles, write_reports
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests
from suite_native import Toolchain, method_bytes, native_method, run_in_fixtures

SOURCE = HERE / "upstream/Lib/test/test_free_threading/test_functools.py"
OWNER = "TestLRUCache"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + flag, type=Path, required=True)
    parser.add_argument("--compiler-commit", required=True)
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or not sysconfig.get_config_var("Py_GIL_DISABLED") or sys._is_gil_enabled():
        parser.error("the upstream package requires a free-threaded CPython 3.14.5 with the GIL disabled")
    manifest = json.loads((HERE / "FREE_THREADING.json").read_text())
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    reference = json.loads((HERE / "reference-free-threading-linux.json").read_text())
    module = manifest["module"]
    variant = "CPython 3.14.5 free-threading build, GIL disabled; original working-threading class decorator"
    discovered = [(c.id(), variant) for c in cases(load_suite(module))]
    # Preserve every original import in source order. Both helpers run Barrier,
    # Thread, random.randint, an actual cache, and the upstream start_threads context.
    imports, _ = SOURCE.read_bytes().split(b"@threading_helper.requires_working_threading()", 1)
    reports = []
    for level in (0, 3):
        tools = Toolchain(args.imre, args.runtime, args.clang, level)
        obs = {}
        for key in occurrence_keys(n for n, _ in discovered):
            name, _ = key
            helper = "_" + name.rsplit(".", 1)[1].rsplit("_", 1)[0]
            helper_bytes = method_bytes(SOURCE, OWNER, helper)
            fixture = imports + b"class NativeCase(unittest.TestCase):\n" + helper_bytes
            proof = {"compiler_commit": args.compiler_commit, "opt_level": level,
                     "method_sha256": hashlib.sha256(method_bytes(SOURCE, OWNER, name.rsplit(".", 1)[1])).hexdigest(),
                     "worker_method": helper, "worker_sha256": hashlib.sha256(helper_bytes).hexdigest()}
            with TemporaryDirectory() as tmp:
                try:
                    run = native_method(tools, Path(tmp), SOURCE, OWNER, name, fixture.decode())
                except RuntimeError as exc:
                    reason = str(exc).replace(tmp, "<case>").strip()
                    obs[key] = {"outcome": "blocked", "reason": reason, "evidence": proof,
                                "roles": roles(body="unobserved", assertions="unobserved", operation="unobserved", fixture="unobserved")}
                    continue
                evidence, failures, skips = run_in_fixtures(load_suite(module), {name: run})
                if skips:
                    raise ValueError(f"unexpected fixture skip: {skips}")
                loc = roles(body="imre", assertions="none", operation="imre", fixture="mixed")
                if failures:
                    obs[key] = dict(outcome="error", reason=failures[0][1], roles=loc, evidence=proof)
                    continue
                entry, = evidence[name]
                proof.update(operation="Native worker threads exercise the same native LRU cache.", fixture=entry["fixture_execution"])
                obs[key] = dict(outcome="pass", roles=loc, evidence=proof)
        reports.append(build_report(reference, level, discovered, obs))
    write_reports(args.result, reports, reference)


if __name__ == "__main__":
    main()
