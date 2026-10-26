"""Record which generation performed the operation in six original subprocess cases."""

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from nested_parents import NAMES, SOURCE
from suite_evidence import classification, validate_pair, write_reports
from suite_native import method_bytes


def apply(reports, measured, reference):
    validate_pair(reports, reference)
    assert measured["schema"] == "imre.subprocess.nested-parents/v3"
    by_level = {r["opt_level"]: r for r in measured["reports"]}
    assert by_level.keys() == {0, 3} and len(measured["reports"]) == 2
    ids = {
        f"test.test_subprocess.{cls}.{name}"
        for cls in ("ProcessTestCase", "ProcessTestCaseNoPoll")
        for name in NAMES
    }
    for report in reports:
        raw = by_level[report["opt_level"]]
        assert (
            raw["cpython_commit"] == reference["cpython_commit"]
            and raw["rows"].keys() == ids
        )
        for row in report["records"]:
            if row["id"] not in ids:
                continue
            name = row["id"]
            method = name.rsplit(".", 1)[1]
            no_poll = "ProcessTestCaseNoPoll" in name
            case = raw["rows"][name]
            assert row["occurrence"] == 1 and row["reference"] == "pass"
            assert (
                case["outcome"] == "pass" and case["original_output_assertions_passed"]
            )
            assert case["roles"] == dict(
                body="cpython",
                assertions="cpython",
                fixture="cpython",
                operation="imre",
            )
            assert (
                case["method_sha256"]
                == hashlib.sha256(
                    method_bytes(SOURCE, "ProcessTestCase", method)
                ).hexdigest()
            )
            stages = case["processes"]
            assert stages.keys() == {"outer", "middle", "data_child"}
            for parent, child in (("outer", "middle"), ("middle", "data_child")):
                assert (
                    stages[parent]["engine"] == "imre"
                    and stages[parent]["created_and_reaped"] == child
                )
            assert all(stage["observed_execve"] for stage in stages.values())
            data = stages["data_child"]["engine"]
            assert data == (
                "cpython" if method == "test_stdout_filedes_of_stdout" else "imre"
            )
            syscalls = stages["outer"]["selector_syscalls"]
            expected = {"select", "pselect6"} if no_poll else {"poll", "ppoll"}
            assert (
                syscalls
                and set(syscalls) <= expected
                and all(c > 0 for c in syscalls.values())
            )
            assert case["fixture_selector"] == (
                "SelectSelector" if no_poll else "PollSelector"
            )
            row.pop("reason", None)
            row.update(
                outcome="pass",
                roles=case["roles"],
                evidence=dict(
                    imre_revision=measured["imre_revision"],
                    tested_tree=measured["tested_tree"],
                    source="nested-chain-linux.json",
                    operation="outer and middle Imre parents each created and reaped the next process; "
                    + (
                        "the middle parent called subprocess with stdout=1; its CPython data child only wrote the bytes"
                        if data == "cpython"
                        else "the innermost data child also ran its original program natively"
                    ),
                    fixture=f"unchanged host assertions, LIFO cleanups and {case['fixture_selector']} binding; outer native syscalls {syscalls}",
                    native_body_diagnostic=case["body_diagnostic"],
                    method_sha256=case["method_sha256"],
                ),
            )
            row["classification"] = classification(row)
        report["counts"] = dict(
            sorted(Counter(row["classification"] for row in report["records"]).items())
        )
    validate_pair(reports, reference)
    return reports


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ("previous", "nested-chain", "result"):
        parser.add_argument("--" + key, type=Path, required=True)
    args = parser.parse_args()
    ref = json.loads((HERE / "reference-linux.json").read_text())
    reports = apply(
        json.loads(args.previous.read_text()),
        json.loads(args.nested_chain.read_text()),
        ref,
    )
    write_reports(args.result, reports, ref)
    for report in reports:
        print(report["opt_level"], report["denominator"], report["counts"])
