"""Apply measured public Popen observations to the complete versioned reports."""

import argparse
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from launch_errno_originals import IDS
from suite_evidence import build_report, validate_pair, write_reports


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--previous", type=Path, required=True)
    p.add_argument("--launch-errno", type=Path, required=True)
    p.add_argument("--result", type=Path, required=True)
    args = p.parse_args()
    previous, launch = (
        json.loads(path.read_text()) for path in (args.previous, args.launch_errno)
    )
    ref = json.loads((HERE / "reference-linux.json").read_text())
    validate_pair(previous, ref)
    if {report["opt_level"] for report in launch["reports"]} != {0, 3} or len(
        launch["reports"]
    ) != 2:
        raise ValueError("need both measured optimizations")
    result = []
    for before in previous:
        level = before["opt_level"]
        [measured] = [
            report for report in launch["reports"] if report["opt_level"] == level
        ]
        if (
            measured["rows"].keys() != IDS
            or measured["cpython_commit"] != ref["cpython_commit"]
        ):
            raise ValueError("incomplete or different upstream launch error suite")
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
            [case] = measured["rows"][name]
            op, body = case["observed"], case["body_attempt"]
            if (
                occ != 1
                or row["reference"] != "pass"
                or op["errno"] != 2
                or op["filename2"] is not None
                or not op["filename"]
                or not op["strerror"]
                or not op["failed_child_reaped"]
                or op["native_parent_selector_calls"]
                or case["roles"]
                != dict(
                    body="cpython",
                    assertions="cpython",
                    operation="imre",
                    fixture="cpython",
                )
            ):
                raise ValueError(f"incomplete public errno observations: {name}")
            if body["status"] != "blocked":
                raise ValueError(
                    f"review newly completed native body separately: {name}"
                )
            observations[key] = dict(
                outcome="pass",
                roles=case["roles"],
                evidence=dict(
                    opt_level=level,
                    imre_revision=launch["imre_revision"],
                    tested_tree=launch["tested_tree"],
                    operation=f"{op['api']}: native child {op['syscall']} failed on {op['filename']!r}; "
                    "the native parent reaped it and returned the original errno, strerror and filename",
                    fixture="original CPython module/class/instance hooks, unchanged method and assertions; "
                    f"{case['fixture_selector']} installed by host, no parent selector syscall before error",
                    native_body_diagnostic=body["reason"],
                    source="launch-errno-linux.json",
                ),
            )
        result.append(
            build_report(ref, level, discovery, observations, exceptions=exceptions)
        )
    write_reports(args.result, result, ref)
    for report in result:
        print(report["opt_level"], report["denominator"], report["counts"])


if __name__ == "__main__":
    main()
