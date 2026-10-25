"""Update only the two observed timeout occurrences, preserving the full reference."""

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import sys


HERE = Path(__file__).resolve().parent


def main():
    sys.path[:0] = [str(HERE), str(HERE.parents[1])]
    from attempt_bodies import SOURCE
    from call_timeout_originals import ASSERTION, IDS
    from suite_evidence import classification, validate_pair, write_reports
    from suite_native import method_bytes

    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("previous", "receipt", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    reports = json.loads(args.previous.read_text())
    reference = json.loads((HERE / "reference-linux.json").read_text())
    receipt = json.loads(args.receipt.read_text())
    validate_pair(reports, reference)
    digest = hashlib.sha256(
        method_bytes(SOURCE, "ProcessTestCase", "test_call_timeout")
    ).hexdigest()
    assert [r["opt_level"] for r in receipt["reports"]] == [0, 3]
    assert receipt["reports"][0]["toolchain"] == receipt["reports"][1]["toolchain"]
    for report in reports:
        level = report["opt_level"]
        (captured,) = (r for r in receipt["reports"] if r["opt_level"] == level)
        assert (
            captured["cpython_commit"] == reference["cpython_commit"]
            and captured["rows"].keys() == IDS
        )
        changed = []
        for row in report["records"]:
            name = row["id"]
            if name not in IDS:
                continue
            case = captured["rows"][name]
            pos, neg = case["timed_out"], case["immediate_exit"]
            assert (
                row["reference"] == "pass"
                and row["occurrence"] == 1
                and "exception" not in row
            )
            assert (
                case["method_sha256"] == digest
                and ASSERTION in case["native_assertion_failure"]
            )
            (kill,) = pos["signals"]
            assert f"kill({pos['child']}, SIGKILL)" in kill and kill.endswith("= 0")
            assert "WTERMSIG(s) == SIGKILL" in pos["reap"] and pos["reap"].endswith(
                f"= {pos['child']}"
            )
            assert not neg["signals"] and "WEXITSTATUS(s) == 0" in neg["reap"]
            assert neg["reap"].endswith(f"= {neg['child']}")
            row.pop("reason", None)
            row.update(
                outcome="pass",
                roles=dict(
                    body="imre", assertions="imre", operation="imre", fixture="mixed"
                ),
                evidence=dict(
                    opt_level=level,
                    source=args.receipt.name,
                    imre_revision=receipt["imre_revision"],
                    tested_tree=receipt["tested_tree"],
                    method_sha256=digest,
                    operation="Compiled native parent launches the requested interpreter, times out, sends SIGKILL and reaps that PID. Substituting /bin/true makes the native assertRaises fail.",
                    fixture="Original CPython module, class and instance hooks; unchanged body and assertion in the compiled native instance.",
                ),
            )
            row["classification"] = classification(row)
            changed.append(name)
        assert len(changed) == len(IDS) and set(changed) == IDS
        report["counts"] = dict(
            sorted(Counter(r["classification"] for r in report["records"]).items())
        )
    write_reports(args.result, reports, reference)


if __name__ == "__main__":
    main()
