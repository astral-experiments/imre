"""Validate and summarize the committed per-occurrence OS, POSIX, and genericpath reports."""
import argparse
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import validate_pair


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--result", type=Path, help="write all three validated pairs in a single JSON object")
    args = parser.parse_args()
    combined = {}
    for suite in ("os", "posix", "genericpath"):
        reference = json.loads((HERE / f"reference-{suite}-linux.json").read_bytes())
        reports = json.loads((HERE / f"evidence-{suite}-linux.json").read_bytes())
        validate_pair(reports, reference)
        combined[suite] = reports
        for report in reports:
            print(f"{report['suite']} -O{report['opt_level']}: {report['denominator']}", report["counts"])
    if args.result:
        args.result.write_text(json.dumps(combined) + "\n")


if __name__ == "__main__":
    main()
