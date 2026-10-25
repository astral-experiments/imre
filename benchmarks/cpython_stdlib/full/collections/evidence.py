"""Reconcile the collections routine, Counter and doctest gates by original test ID."""

import argparse
import csv
from collections import Counter
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
CATEGORIES = ("native-body/native-assertions", "native-body/host-fixtures",
              "native-body/host-output-check", "host-driven-native", "mixed",
              "reference-only", "compile-blocked", "fixture-blocked", "failed", "reference-skip")


def paired(paths, field, commit):
    reports = [json.loads(path.read_text()) for path in paths]
    if {r["opt_level"] for r in reports} != {0, 3} or any(r["cpython_commit"] != commit for r in reports):
        raise ValueError(f"need both levels at the pinned CPython commit: {paths}")
    if reports[0][field] != reports[1][field]:
        raise ValueError(f"optimization levels differ; retain and inspect both reports: {paths}")
    return reports[0][field]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("routines", "counter", "doctests"):
        parser.add_argument("--" + name, type=Path, nargs=2, required=True)
    parser.add_argument("--result", type=Path, required=True)
    args = parser.parse_args()
    reference = json.loads((HERE / "reference-linux.json").read_text())
    commit = reference["cpython_commit"]
    routines = paired(args.routines, "routines", commit)
    counter = paired(args.counter, "tests", commit)
    doctests = paired(args.doctests, "tests", commit)
    if set(routines) != set(reference["reference"]) or not (counter.keys() | doctests.keys()) <= routines.keys():
        raise ValueError("evidence does not match the pinned collections inventory")
    rows = []
    for case_id, routine in routines.items():
        category = routine["result"]
        source = routine.get("source", "")
        location = routine.get("location", "")
        detail = routine.get("reason", "")
        if case_id in counter:
            result = counter[case_id]
            if result["status"] == "pass":
                category = "native-body/native-assertions"
                location = ""
                detail = f"{result['assertions']} native assertions; method SHA-256 {result['method_sha256']}"
                if routine["result"] == "ran-pending-operation-audit":
                    detail += "; also passed under the original host fixtures"
            else:
                category, detail = "failed", str(result)
        elif case_id in doctests:
            result = doctests[case_id]
            category = result["status"]
            detail = result.get("diagnostic", f"{result['examples']} original doctest prompts")
        elif category == "ran-pending-operation-audit":
            category = "reference-only"
            if case_id in {f"test.test_deque.TestSequence.{method}" for method in
                           ("test_getitem", "test_getslice", "test_subscript")}:
                detail = "Original body is pass; native execution has no collections operation."
            else:
                detail = "Body ran under host fixtures; module operation has not been audited."
        if category not in CATEGORIES:
            raise ValueError(f"unclassified outcome for {case_id}: {category}")
        rows.append((case_id, category, source, location, detail))
    with args.result.open("w", newline="") as out:
        writer = csv.writer(out, delimiter="\t", lineterminator="\n")
        writer.writerow(("upstream_id", "execution_at_O0_and_O3", "method_source", "diagnostic_location", "evidence"))
        writer.writerows(sorted(rows))
    counts = Counter(row[1] for row in rows)
    for category in CATEGORIES:
        print(f"{category}: {counts[category]}")
    print(f"total: {sum(counts.values())}")


if __name__ == "__main__":
    main()
