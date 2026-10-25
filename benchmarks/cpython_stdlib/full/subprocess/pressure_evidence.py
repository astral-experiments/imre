"""Join the two descriptor-pressure observations without changing other occurrences."""

from collections import Counter
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from pressure_fixture import IDS
from suite_evidence import classification, validate_pair, write_reports


def update(previous, observed, reference):
    validate_pair(previous, reference)
    if sorted(report["opt_level"] for report in observed["reports"]) != [0, 3]:
        raise ValueError("missing pressure run")
    for report in previous:
        level = report["opt_level"]
        [measured] = [r for r in observed["reports"] if r["opt_level"] == level]
        if (
            measured["cpython_commit"] != reference["cpython_commit"]
            or measured["rows"].keys() != IDS
        ):
            raise ValueError("pressure suite does not match the pinned original IDs")
        for row in report["records"]:
            if row["id"] not in IDS:
                continue
            [case] = measured["rows"][row["id"]]
            if (
                row["occurrence"] != 1
                or row["reference"] != "pass"
                or case["free_slots"] != 10
                or case["pressure_fds"] + 13 != case["open_file_limit"]
                or case["native_parents"] != 1
                or not case["original_descriptor_numbers_and_cloexec"]
                or not case["native_creations"]
                == case["native_reaps"]
                == case["unchanged_fd_table_after_each_communicate"]
                == 15
                or not case["selector_calls"]
                or case["roles"]
                != dict(
                    body="cpython",
                    assertions="cpython",
                    operation="imre",
                    fixture="cpython",
                )
            ):
                raise ValueError(("incomplete pressure operation", row["id"]))
            want = (
                {"select", "pselect6"}
                if "ProcessTestCaseNoPoll." in row["id"]
                else {"poll", "ppoll"}
            )
            if case["selector_calls"].keys() - want:
                raise ValueError(("wrong selector", row["id"]))
            assert case["body_attempt"]["status"] == "blocked"
            row.pop("reason", None)
            row.update(
                outcome="pass",
                roles=case["roles"],
                evidence=dict(
                    opt_level=level,
                    imre_revision=observed["subprocess_revision"],
                    tested_tree=observed["tested_tree"],
                    operation="one native parent launched 15 three-pipe children and communicated with the original input; "
                    "each child exited zero and the entire original descriptor table was unchanged after every wait",
                    fixture=f"unchanged CPython body, assertions and hooks; {case['fixture_selector']} in the native parent; "
                    f"{case['pressure_fds']} original file descriptions, original numbers and CLOEXEC, ten free slots",
                    native_body_diagnostic=case["body_attempt"]["reason"],
                    source="pressure-replay-linux.json",
                ),
            )
            row["classification"] = classification(row)
        report["counts"] = dict(
            sorted(Counter(r["classification"] for r in report["records"]).items())
        )
    validate_pair(previous, reference)


if __name__ == "__main__":
    import argparse

    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("previous", type=Path)
    p.add_argument("result", type=Path)
    args = p.parse_args()
    read = lambda path: json.loads(path.read_text())
    previous, reference = read(args.previous), read(HERE / "reference-linux.json")
    update(previous, read(HERE / "pressure-replay-linux.json"), reference)
    write_reports(args.result, previous, reference)
