"""Apply only the observed restore_signals occurrence to each full report."""

# ruff: noqa: E402 -- the shared evidence validator lives two directories above this suite.

import argparse
from collections import Counter
import json
from pathlib import Path
import signal
import sys

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from restore_signals_original import ID
from suite_evidence import classification, validate_pair, write_reports


def apply(reports, receipt, reference):
    validate_pair(reports, reference)
    assert receipt["schema"] == "imre.subprocess.restore-signals/v1"
    raw = {r["opt_level"]: r for r in receipt["reports"]}
    assert set(raw) == {0, 3} and len(receipt["reports"]) == 2
    bits = (1 << (signal.SIGPIPE - 1)) | (1 << (signal.SIGXFSZ - 1))
    for report in reports:
        observed = raw[report["opt_level"]]
        assert observed["cpython_commit"] == reference["cpython_commit"]
        assert observed["rows"].keys() == {ID}
        [case] = observed["rows"][ID]
        assert case["body_attempt"]["status"] == "blocked"
        assert case["roles"] == dict(
            body="cpython", assertions="cpython", operation="imre", fixture="cpython"
        )
        assert case["argv"] == ["cat", "/proc/self/status"]
        first, second = case["children"]
        assert first["restore_signals"] is False and second["restore_signals"] is True
        assert all(
            c["ppid"] == case["native_parent"] and c["successful_parent_reaps"] == 1
            for c in (first, second)
        )
        assert len({first["pid"], second["pid"], case["native_parent"]}) == 3
        a, b = (int(c["sigign"], 16) for c in (first, second))
        assert a & bits == bits and b & bits == 0 and a ^ b == bits
        [row] = [r for r in report["records"] if r["id"] == ID]
        assert row["reference"] == "pass" and row["occurrence"] == 1
        row.pop("reason", None)
        row.update(
            outcome="pass",
            roles=case["roles"],
            evidence=dict(
                imre_revision=receipt["imre_revision"],
                tested_tree=receipt["tested_tree"],
                source="restore-signals-linux.json",
                native_body_diagnostic=case["body_attempt"]["reason"],
                operation="one compiled Imre parent launched and reaped both cat children; their /proc/self/status ignored masks differed exactly by SIGPIPE and SIGXFSZ",
                fixture="unchanged CPython method, assertions and hooks; system cat read its own kernel status",
            ),
        )
        row["classification"] = classification(row)
        report["counts"] = dict(
            sorted(Counter(r["classification"] for r in report["records"]).items())
        )
    validate_pair(reports, reference)
    return reports


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("previous", "receipt", "result"):
        p.add_argument("--" + name, type=Path, required=True)
    args = p.parse_args()
    reference = json.loads((HERE / "reference-linux.json").read_text())
    reports = apply(
        json.loads(args.previous.read_text()),
        json.loads(args.receipt.read_text()),
        reference,
    )
    write_reports(args.result, reports, reference)
