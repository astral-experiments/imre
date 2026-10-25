"""Verify the original waitpid mock and the separate real SIGQUIT controls."""

import argparse
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import build_report, validate_pair, write_reports  # noqa: E402

NAME = "test.test_subprocess.POSIXProcessTestCase.test_stopped"


def verified_shared_wait(shared, level, roles):
    [pair] = [p for p in shared["reports"] if p["opt_level"] == level]
    for key, outcome, failure in (
        ("mock_only", "failure", "0 != -3"),
        ("integrated", "pass", None),
        ("hosted_integrated", "pass", None),
    ):
        report = pair[key]
        [case] = report["rows"][NAME]
        assert report["opt_level"] == level and case["roles"] == roles
        assert (case["outcome"], case["failure"]) == (outcome, failure)
        assert (
            case["external_wait_in_native_parent"]
            and case["original_support_wait_process"]
        )
        # The older runtime blocks; the concurrent-wait implementation polls.
        flags = 0 if key == "mock_only" else 1
        assert case["native_waitpid_mock"] == [dict(options=flags, status=895)]
        assert case["restoration"] == dict(
            original_host_binding_restored=True,
            shared_native_mock_calls=1,
            native_post_scope_wait_errno=10,
        )
        assert case["native_child_exit"] == case["native_signals_sent"] == 0
    assert pair["integrated"]["toolchain"] == pair["hosted_integrated"]["toolchain"]
    return pair["integrated"]["rows"][NAME][0]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--previous", required=True, type=Path)
    p.add_argument("--result", required=True, type=Path)
    args = p.parse_args()
    ref = json.loads((HERE / "reference-linux.json").read_text())
    prior = json.loads(args.previous.read_text())
    validate_pair(prior, ref)
    data = json.loads((HERE / "stopped-status-linux.json").read_text())
    shared = json.loads((HERE / "shared-mock-linux.json").read_text())
    assert shared["cpython_commit"] == ref["cpython_commit"]
    assert data["cpython_commit"] == ref["cpython_commit"] and [
        r["opt_level"] for r in data["reports"]
    ] == [0, 3]
    out = []
    for before in prior:
        level = before["opt_level"]
        [pair] = [x for x in data["reports"] if x["opt_level"] == level]
        original = pair["original"]
        assert (
            original["cpython_commit"] == ref["cpython_commit"]
            and original["opt_level"] == level
        )
        [case] = original["rows"][NAME]
        roles = dict(
            body="cpython", assertions="cpython", operation="imre", fixture="mixed"
        )
        checked = verified_shared_wait(shared, level, roles)
        assert (
            case["roles"] == roles
            and case["outcome"] == "pass"
            and case["failure"] is None
        )
        assert (
            case["body_attempt"]["status"] == "blocked"
            and case["original_support_wait_process"]
        )
        assert case["external_wait_in_native_parent"] and case["native_child_exit"] == 0
        assert (
            case["native_waitpid_mock"] == [dict(options=1, status=895)]
            and not case["public_wait_observed_ECHILD"]
        )
        assert case["native_signals_sent"] == 0 and original[
            "positive_live_child"
        ] == dict(kernel_kill="SIGTERM", wait_status=-15)
        for label, values in (("before", [0, 0, 7]), ("after", [-3, -3, 7])):
            control = pair[label]
            assert control["opt_level"] == level and set(control["modes"]) == {
                "wait",
                "poll",
            }
            for observed in control["modes"].values():
                assert observed["parent"] == "imre" and observed["expected"] == [
                    -3,
                    -3,
                    7,
                ]
                assert (
                    observed["observed"] == values
                    and observed["native_stop_signal"] == 3
                    and observed["post_resume_reap"] == 7
                )
        obs, exceptions, discovery = {}, {}, []
        for row in before["records"]:
            key = row["id"], row["occurrence"]
            discovery.append((key[0], row["fixture_variant"]))
            if "exception" in row:
                exceptions[*key, level] = row["exception"]
            obs[key] = {
                k: row[k]
                for k in ("outcome", "roles", "reason", "evidence")
                if k in row
            }
            if key == (NAME, 1):
                assert row["reference"] == "pass"
                obs[key] = dict(
                    outcome="pass",
                    roles=roles,
                    evidence=dict(
                        opt_level=level,
                        imre_revision=shared["integrated"]["revision"],
                        tested_tree=shared["integrated"]["tested_tree"],
                        source="shared-mock-linux.json",
                        operation="Native Popen.wait called the shared Mock, consumed the original patch's raw stop status and returned -3. A fresh wait after restoration reached the kernel and reported ECHILD.",
                        fixture="CPython owns the original body, patch context, tuple and assertions. The compiled parent's scoped libc fixture invoked Imre's unittest.mock recorder during the actual wait. The direct native mock.patch expression remains blocked.",
                        native_body_diagnostic=checked["body_attempt"]["reason"],
                    ),
                )
        out.append(build_report(ref, level, discovery, obs, exceptions=exceptions))
    write_reports(args.result, out, ref)
    for r in out:
        print(r["opt_level"], r["denominator"], r["counts"])


if __name__ == "__main__":
    main()
