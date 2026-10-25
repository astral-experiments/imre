"""Apply the two observed host-driven finalizers to the full Linux report."""

import argparse
from collections import Counter
import errno
import json
from pathlib import Path
import signal
import sys

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from active_originals import IDS, hash_file
from suite_evidence import classification, validate_pair, write_reports


def apply(previous, measured, reference):
    validate_pair(previous, reference)
    assert measured["schema"] == "imre.subprocess.active-children/v1"
    reports = {report["opt_level"]: report for report in measured["reports"]}
    assert len(measured["reports"]) == 2 and reports.keys() == {0, 3}
    for before in previous:
        level = before["opt_level"]
        raw = reports[level]
        assert (
            raw["rows"].keys() == IDS
            and raw["cpython_commit"] == reference["cpython_commit"]
        )
        assert raw["fixture_os_sha256"] == hash_file(HERE / "fixture_active_os.c")
        assert raw["parent_source_sha256"] == hash_file(HERE / "active_parent.py")
        for row in before["records"]:
            name = row["id"]
            if name not in IDS:
                continue
            assert row["reference"] == "pass" and row["occurrence"] == 1
            [case] = raw["rows"][name]
            killed = name.endswith("_killed")
            obs = case["observed"]
            assert (
                obs["parent_executable_verified"] and obs["parent_warning_pid_matched"]
            )
            assert (
                obs["original_object_identity_retained"]
                and obs["applied_host_filter"] == "always"
            )
            assert obs["data_child_argv"][:2] == ["<pinned-cpython>", "-c"]
            assert obs["membership_lengths"] == ([1, 0, 0, 0] if killed else [1, 1, 0])
            assert obs["successful_kernel_reaps"] == 1 and obs["kernel_status"] == (
                -int(signal.SIGKILL) if killed else 0
            )
            assert obs["fixture_waitpid_errno"] == (errno.ECHILD if killed else None)
            assert obs["failed_launch_errno"] == (errno.ENOENT if killed else None)
            assert (
                obs["fixture_closed_parent_pipe_ends"] == 2
                and obs["fixture_cleanup_calls"] == 1
            )
            assert case["body_attempt"]["status"] == "blocked"
            assert case["roles"] == dict(
                body="cpython", assertions="cpython", fixture="mixed", operation="imre"
            )
            row.pop("reason", None)
            row.update(
                outcome="pass",
                roles=case["roles"],
                evidence=dict(
                    imre_revision=measured["imre_revision"],
                    tested_tree=measured["tested_tree"],
                    source="active-children-linux.json",
                    operation="native parent: finalizer warned for actual child PID, retained original Popen in _active, "
                    + (
                        "reaped SIGKILL before the subsequent failed exec; fixture waitpid returned ECHILD"
                        if killed
                        else "original teardown waited on the retained child (exit 0) and _cleanup removed it"
                    ),
                    fixture="unchanged CPython body, assertions, warning capture and teardown; two pipe closes, "
                    "_active and _cleanup delegated to their native owner; literal Python data child ran on pinned CPython",
                    native_body_diagnostic=case["body_attempt"]["reason"],
                ),
            )
            row["classification"] = classification(row)
        before["counts"] = dict(
            sorted(Counter(row["classification"] for row in before["records"]).items())
        )
    validate_pair(previous, reference)
    return previous


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("previous", "active-children", "result"):
        p.add_argument("--" + name, type=Path, required=True)
    args = p.parse_args()
    ref = json.loads((HERE / "reference-linux.json").read_text())
    reports = apply(
        json.loads(args.previous.read_text()),
        json.loads(args.active_children.read_text()),
        ref,
    )
    write_reports(args.result, reports, ref)
    for report in reports:
        print(report["opt_level"], report["denominator"], report["counts"])
