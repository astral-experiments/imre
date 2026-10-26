"""Overlay the four observed native pipe class checks on the full Linux reports."""

import argparse
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from pipe_class_originals import IDS
from suite_evidence import build_report, validate_pair, write_reports


def apply(previous, measured, reference):
    validate_pair(previous, reference)
    assert measured["schema"] == "imre.subprocess.pipe-classes/v1"
    assert measured["platform"] == "linux"
    reports = {r["opt_level"]: r for r in measured["reports"]}
    assert len(measured["reports"]) == 2 and reports.keys() == {0, 3}
    result = []
    for before in previous:
        level = before["opt_level"]
        raw = reports[level]
        assert raw["cpython_commit"] == reference["cpython_commit"]
        assert raw["rows"].keys() == IDS
        observations, exceptions, discovery = {}, {}, []
        for row in before["records"]:
            name, occ = key = (row["id"], row["occurrence"])
            discovery.append((name, row["fixture_variant"]))
            if "exception" in row:
                exceptions[name, occ, level] = row["exception"]
            observations[key] = {
                k: row[k]
                for k in ("outcome", "roles", "reason", "evidence")
                if k in row
            }
            if name not in IDS:
                continue
            assert occ == 1 and row["reference"] == "pass"
            [case] = raw["rows"][name]
            obs, attempt = case["observed"], case["body_attempt"]
            expected = (
                "RawIOBase" if name.endswith("unbuffered_works") else "BufferedIOBase"
            )
            selector = "SelectSelector" if "CaseNoPoll." in name else "PollSelector"
            assert obs["native_class"] == expected and obs["native_io_base"] is True
            assert obs["class_unchanged_after_close"] is True
            assert obs["streams"] == ["stdin", "stdout", "stderr"]
            assert (
                obs["closed_native_descriptors"] == 3
                and obs["successful_native_reaps"] == 1
            )
            assert obs["native_selector_syscalls"] == 0 and obs["data_child_argv"]
            assert (
                case["fixture_selector"] == selector and attempt["status"] == "blocked"
            )
            assert case["roles"] == dict(
                body="cpython",
                assertions="cpython",
                operation="imre",
                fixture="cpython",
            )
            observations[key] = dict(
                outcome="pass",
                roles=case["roles"],
                evidence=dict(
                    opt_level=level,
                    imre_revision=measured["imre_revision"],
                    tested_tree=measured["tested_tree"],
                    source="pipe-classes-linux.json",
                    operation=f"compiled isinstance on stdin/stdout/stderr reported {expected} and IOBase before and after close; native parent closed three distinct descriptors and reaped the fixture child once",
                    fixture=f"unchanged method, assertions and hooks on CPython; host {selector}, no native selector syscall",
                    native_body_diagnostic=attempt["reason"],
                ),
            )
        result.append(
            build_report(
                reference, level, discovery, observations, exceptions=exceptions
            )
        )
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ("previous", "pipe-classes", "result"):
        parser.add_argument("--" + arg, type=Path, required=True)
    args = parser.parse_args()
    ref = json.loads((HERE / "reference-linux.json").read_text())
    result = apply(
        json.loads(args.previous.read_text()),
        json.loads(args.pipe_classes.read_text()),
        ref,
    )
    write_reports(args.result, result, ref)
    for report in result:
        print(report["opt_level"], report["denominator"], report["counts"])


if __name__ == "__main__":
    main()
