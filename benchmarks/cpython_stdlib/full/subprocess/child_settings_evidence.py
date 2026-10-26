"""Apply the three child-setting observations to the paired Linux reports."""

import argparse
from collections import Counter
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from child_settings_originals import IDS
from suite_evidence import classification, validate_pair, write_reports


def apply(previous, measured, reference):
    validate_pair(previous, reference)
    assert measured["schema"] == "imre.subprocess.child-settings/v1"
    levels = {r["opt_level"]: r for r in measured["reports"]}
    assert set(levels) == {0, 3} and len(measured["reports"]) == 2
    for before in previous:
        raw = levels[before["opt_level"]]
        assert (
            raw["rows"].keys() == IDS
            and raw["cpython_commit"] == reference["cpython_commit"]
        )
        for row in before["records"]:
            name = row["id"]
            if name not in IDS:
                continue
            assert row["reference"] == "pass" and row["occurrence"] == 1
            [case] = raw["rows"][name]
            observed = case["observed"]
            expected = {
                "test_start_new_session": (
                    "start_new_session",
                    True,
                    "setsid",
                    "getsid",
                ),
                "test_process_group_0": ("process_group", 0, "setpgid", "getpgid"),
                "test_umask": ("umask", 0o053, "umask", "openat"),
            }[name.rsplit(".", 1)[1]]
            assert (
                tuple(
                    observed[k]
                    for k in (
                        "option",
                        "value",
                        "pre_exec_syscall",
                        "child_observation",
                    )
                )
                == expected
            )
            assert (
                observed["successful_parent_reaps"] == 1
                and observed["data_child"] == "pinned CPython"
            )
            assert case["roles"] == dict(
                body="cpython",
                assertions="cpython",
                operation="imre",
                fixture="cpython",
            )
            assert case["body_attempt"]["status"] == "blocked"
            mask = expected[0] == "umask"
            assert observed["output_file_mode"] == (0o624 if mask else None)
            assert observed["child_id_equals_pid"] == (not mask)
            row.pop("reason", None)
            row.update(
                outcome="pass",
                roles=case["roles"],
                evidence=dict(
                    imre_revision=measured["imre_revision"],
                    tested_tree=measured["tested_tree"],
                    source="child-settings-linux.json",
                    operation=f"native {observed['api']}: child called {expected[2]} before exec; "
                    + (
                        "its O_CREAT file had mode 0624"
                        if mask
                        else f"{expected[3]} returned its own PID, different from the parent's ID"
                    )
                    + "; the native parent reaped it once",
                    fixture="unchanged CPython method, assertions and hooks; original Python data child on pinned CPython",
                    native_body_diagnostic=case["body_attempt"]["reason"],
                ),
            )
            row["classification"] = classification(row)
        before["counts"] = dict(
            sorted(Counter(r["classification"] for r in before["records"]).items())
        )
    validate_pair(previous, reference)
    return previous


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ("previous", "child-settings", "result"):
        parser.add_argument("--" + key, required=True, type=Path)
    args = parser.parse_args()
    ref = json.loads((HERE / "reference-linux.json").read_text())
    result = apply(
        json.loads(args.previous.read_text()),
        json.loads(args.child_settings.read_text()),
        ref,
    )
    write_reports(args.result, result, ref)
    for report in result:
        print(report["opt_level"], report["denominator"], report["counts"])
