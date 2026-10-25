"""Record the verified reap/signal race once per level in the existing full denominator."""

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import sys


def main():
    here = Path(__file__).resolve().parent
    sys.path[:0] = [str(here), str(here.parents[1])]
    from instance_poll_originals import NAME
    from suite_evidence import classification, validate_pair, write_reports
    from suite_native import method_bytes

    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("previous", "receipt", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    previous = json.loads(args.previous.read_text())
    reference = json.loads((here / "reference-linux.json").read_text())
    validate_pair(previous, reference)
    receipt = json.loads(args.receipt.read_text())
    assert [r["opt_level"] for r in receipt["reports"]] == [0, 3]
    assert receipt["reports"][0]["toolchain"] == receipt["reports"][1]["toolchain"]
    raw = method_bytes(
        here / "upstream/Lib/test/test_subprocess.py",
        "POSIXProcessTestCase",
        "test_send_signal_race2",
    )
    method_hash = hashlib.sha256(raw).hexdigest()
    for report in previous:
        level = report["opt_level"]
        (captured,) = (r for r in receipt["reports"] if r["opt_level"] == level)
        assert (
            captured["id"] == NAME
            and captured["cpython_commit"] == reference["cpython_commit"]
        )
        assert (
            captured["method_sha256"] == method_hash
            and captured["replacement"] == "lambda: None"
        )
        assert captured["body_attempt"]["status"] == "blocked"
        for key in ("installed", "host_only_control"):
            obs = captured[key]
            pid = obs["child"]
            assert (
                obs["native_parent"] != pid and '"-c", "exit(1)"' in obs["child_exec"]
            )
            assert "WEXITSTATUS(s) == 1" in obs["reap"] and obs["reap"].endswith(
                f"= {pid}"
            )
            assert (
                obs["restored_wait"].startswith(f"wait4({pid},")
                and "= -1 ECHILD" in obs["restored_wait"]
            )
            assert obs["fixture_events"] == [
                "install",
                "clear",
                "signal",
                "restore",
                "kill",
            ]
        installed = captured["installed"]
        (kill,) = installed["signals"]
        assert (
            kill.startswith(f"kill({installed['child']}, SIGTERM)")
            and "= -1 ESRCH" in kill
        )
        assert captured["host_only_control"]["signals"] == []
        (row,) = (r for r in report["records"] if r["id"] == NAME)
        assert (
            row["reference"] == "pass"
            and row["occurrence"] == 1
            and "exception" not in row
        )
        row.pop("reason", None)
        row.update(
            outcome="pass",
            roles=dict(
                body="cpython", assertions="cpython", operation="imre", fixture="mixed"
            ),
            evidence=dict(
                opt_level=level,
                source=args.receipt.name,
                imre_revision=receipt["imre_revision"],
                tested_tree=receipt["tested_tree"],
                method_sha256=method_hash,
                native_body_diagnostic=captured["body_attempt"]["reason"],
                operation="Native Popen reaps exit 1, calls the replacement poll inside send_signal, attempts SIGTERM on that same PID (ESRCH), and observes ECHILD after the original scope restores poll. No SIGKILL.",
                fixture="Original CPython method, implicit no-exception outcome and mock.patch.object scope; its exact lambda is installed and removed on the native Popen. Leaving the patch on the host alone yields zero kill syscalls.",
            ),
        )
        row["classification"] = classification(row)
        report["counts"] = dict(
            sorted(Counter(r["classification"] for r in report["records"]).items())
        )
    write_reports(args.result, previous, reference)


if __name__ == "__main__":
    main()
