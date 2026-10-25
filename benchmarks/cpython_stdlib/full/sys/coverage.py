"""Join the full pinned reference with native evidence and individual assessments."""

import argparse
from collections import Counter
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent


def coverage():
    reference = json.loads((HERE / "reference-linux.json").read_text())
    notes = {}
    for line in (HERE / "assessments.tsv").read_text().splitlines():
        if not line or line.startswith("#"):
            continue
        name, state, reason = line.split("\t")
        name = reference["module"] + "." + name
        if name in notes or state not in {"blocked", "failed", "proposed", "reference-only"}:
            raise ValueError(line)
        notes[name] = (state, reason)
    evidence = {}
    for level in (0, 3):
        report = json.loads((HERE / f"native-O{level}.json").read_text())
        if (report["opt_level"] != level or report["denominator"] != reference["denominator"]
                or report["cpython_commit"] != reference["cpython_commit"]):
            raise ValueError("native record refers to a different reference")
        evidence[level] = report["tests"]
    if evidence[0] != evidence[3]:
        raise ValueError("native evidence differs by optimization level; review before crediting")
    if (notes.keys() & evidence[0].keys() or
            (notes.keys() | evidence[0].keys()) - reference["reference"].keys()):
        raise ValueError("duplicate or unknown assessment")

    results = {}
    for name, baseline in reference["reference"].items():
        row = {"reference": baseline}
        if name in evidence[0]:
            item = evidence[0][name]
            if item["status"] == "pass":
                row.update(status=item["execution"], levels=[0, 3], evidence=item)
            elif item["status"] == "upstream-skip":
                row.update(status="reference-skip", reason=item["reason"])
            else:
                raise ValueError((name, item))
        elif name in notes:
            state, reason = notes[name]
            row.update(status="reference-only" if state == "proposed" else state, reason=reason)
            if state == "proposed":
                row["exclusion"] = "proposed only; still in denominator"
        elif isinstance(baseline, dict) and set(baseline) == {"skipped"}:
            row.update(status="reference-skip", reason=baseline["skipped"])
        else:
            raise ValueError(f"missing assessment for {name}")
        results[name] = row
    if len(results) != reference["denominator"]:
        raise ValueError("incomplete coverage")
    counts = Counter(row["status"] for row in results.values())
    categories = ("native-body", "host-driven-native", "mixed", "reference-only",
                  "blocked", "failed", "reference-skip")
    if not counts.keys() <= set(categories):
        raise ValueError(counts)
    return {"cpython_commit": reference["cpython_commit"], "denominator": len(results),
            "counts": {category: counts[category] for category in categories},
            "proposed_exclusions": sum("exclusion" in row for row in results.values()),
            "approved_exclusions": 0, "tests": results}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--result", type=Path, help="write the full per-ID JSON")
    args = parser.parse_args()
    report = coverage()
    if args.result:
        args.result.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({k: v for k, v in report.items() if k != "tests"}, indent=2))
