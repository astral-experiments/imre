"""Summarize the checked-in CPython reference results; these are not native passes."""
import argparse
from collections import Counter
import json
from pathlib import Path
import tomllib

HERE = Path(__file__).resolve().parent
STATUSES = ("pass", "skipped", "failure", "error")


def status(value):
    if value == "pass":
        return "pass"
    if isinstance(value, dict) and len(value) == 1:
        key, reason = next(iter(value.items()))
        if key in STATUSES[1:] and isinstance(reason, str):
            return key
    raise ValueError(f"unrecognized reference outcome: {value!r}")


def occurrences(report):
    if "reference" in report:
        groups = [report["reference"]]
    elif "routines" in report:  # JSON reports fixtures by their defining routine.
        groups = report["routines"].values()
    else:  # pathlib records an upstream skip reason or a successful run per ID.
        for case, reason in report["reference_skip_reason_by_id"].items():
            yield case, "pass" if reason is None else {"skipped": reason}
        return
    for results in groups:
        for case, outcome in results.items():
            for occurrence in outcome if isinstance(outcome, list) else [outcome]:
                yield case, occurrence


def checked_reference(path, pin, platform):
    report = json.loads(path.read_bytes())
    if report["cpython_commit"] != pin or report["platform"] != platform:
        raise ValueError(f"{path}: wrong interpreter pin or platform")
    results = list(occurrences(report))
    counts = Counter(status(outcome) for _, outcome in results)
    expected = report.get("denominator", report.get("executions", report.get("discovered")))
    if len(results) != expected:
        raise ValueError(f"{path}: has {len(results)} executions, expected {expected}")
    for field, measured in [("unique_ids", len({case for case, _ in results})),
                            ("passed", counts["pass"]), ("skipped", counts["skipped"])]:
        if field in report and report[field] != measured:
            raise ValueError(f"{path}: {field}={report[field]}, measured {measured}")
    return report, results


def read_results(path, pin, platform):
    _, results = checked_reference(path, pin, platform)
    return [(case, status(outcome)) for case, outcome in results]


def summary(results):
    counts = Counter(value for _, value in results)
    return {"instances": len(results), "distinct_ids": len({case for case, _ in results}),
            **{key: counts[key] for key in STATUSES}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", default="linux")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    pin = json.loads((HERE.parent / "UPSTREAM.json").read_bytes())["commit"]
    assigned = set(tomllib.loads((HERE / "scope.toml").read_text())["wave_one"])
    # Include future module ledgers even before the work queue is reorganized.
    files = sorted(HERE.glob(f"*/reference*-{args.platform}.json"))
    by_module = {}
    for path in files:
        by_module.setdefault(path.parent.name, []).extend(read_results(path, pin, args.platform))
    missing = sorted(assigned - by_module.keys())
    if missing:
        parser.exit(1, f"missing {args.platform} reference results for: {', '.join(missing)}\n")
    rows = {name: summary(results) for name, results in sorted(by_module.items())}
    total = summary([item for results in by_module.values() for item in results])
    if args.json:
        print(json.dumps({"cpython_commit": pin, "platform": args.platform,
                          "modules": rows, "total": total}, indent=2))
    else:
        print("CPython reference only")
        print(f"{'module':<16}{'instances':>12}{'IDs':>8}{'pass':>8}{'skip':>8}{'fail':>8}{'error':>8}")
        for name, row in [*rows.items(), ("TOTAL", total)]:
            print(f"{name:<16}{row['instances']:>12}{row['distinct_ids']:>8}"
                  f"{row['pass']:>8}{row['skipped']:>8}{row['failure']:>8}{row['error']:>8}")


if __name__ == "__main__":
    main()
