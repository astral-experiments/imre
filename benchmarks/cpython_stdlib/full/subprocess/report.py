"""Combine per-ID observations from both optimization levels without double counting."""

import argparse
from collections import Counter
import json
from pathlib import Path
import re


def load(path):
    return json.loads(path.read_text())


def compact(row):
    result = {"status": row["status"]}
    if "reason" in row:
        reason = re.sub(r"0x[0-9a-f]+", "<address>", row["reason"])
        reason = re.sub(r"/tmp/imre-subprocess-audit-[^ /'\":]+/[^ /'\":]+", "<worker>", reason)
        result["reason"] = reason
    if row.get("operation_calls"):
        result["operations"] = dict(sorted(Counter(row["operation_calls"]).items()))
    return result


REFERENCE_ONLY = {
    "test_exception_errpipe_normal": "the original patches CPython's _fork_exec and Popen.__del__; its simulated errpipe is read by CPython",
    "test_fork_exec": "the original calls CPython's private _posixsubprocess.fork_exec directly",
    "test_fork_exec_sorted_fd_sanity_check": "the original calls CPython's private _posixsubprocess.fork_exec with a mutable __int__ callback",
    "test_preexec_at_exit": "assert_python_ok launches a CPython interpreter; the Popen in __del__ runs in that interpreter",
    "test_failed_child_execute_fd_leak": "the original subclasses CPython Popen, overrides _execute_child, and counts descriptors in the host",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre-revision", required=True)
    for name in ("audit", "bodies", "run-log"):
        parser.add_argument("--" + name, nargs=2, type=Path, required=True, metavar=("O0", "O3"))
    parser.add_argument("--result", type=Path, required=True)
    args = parser.parse_args()
    here = Path(__file__).resolve().parent
    reference = load(here / "reference-linux.json")["reference"]
    ids = {name: {} for name in sorted(reference)}
    totals = {}
    for i, level in enumerate(("0", "3")):
        audit, bodies = load(args.audit[i]), load(args.bodies[i])
        if set(audit["rows"]) != set(reference) or str(audit["opt_level"]) != level or bodies["opt_level"] != level:
            raise ValueError("incomplete audit or wrong optimization level")
        log = args.run_log[i].read_text()
        run_ids = set(re.findall(r"^\w+ \((test\.test_subprocess\.RunFuncTestCase\.\w+)\) \.\.\. ok$", log, re.M))
        if "\nOK\n" not in log or len(run_ids) != 8 or not run_ids <= set(reference):
            raise ValueError("legacy native run evidence is incomplete")
        counts = Counter()
        for name in ids:
            row = compact(audit["rows"][name])
            if row["status"] == "reference-only":
                row["reason"] = REFERENCE_ONLY.get(name.rsplit(".", 1)[1], row["reason"])
            if name in bodies["native_body_host_fixtures"]:
                [evidence] = bodies["native_body_host_fixtures"][name]
                row.update(status="native-body-host-fixtures", method_sha256=evidence["method_sha256"])
            elif name in run_ids:
                row = {"status": "host-driven-native", "operations": {"run": 1},
                       "fixture": "native_subprocess.py reconstructed host case; original method and assertions"}
            if name in bodies["blocked"]:
                row["native_body_blocker"] = bodies["blocked"][name]
            ids[name]["O" + level] = row
            counts[row["status"]] += 1
        totals["O" + level] = dict(sorted(counts.items()))
    for name, pair in ids.items():
        if pair["O0"] == pair["O3"]:
            ids[name] = {"O0/O3": pair["O0"]}
    report = {"imre_revision": args.imre_revision, "denominator": len(ids), "totals": totals, "ids": ids}
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
