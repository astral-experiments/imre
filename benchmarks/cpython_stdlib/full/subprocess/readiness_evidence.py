"""Apply only the verified original raw read; keep its host select in the roles."""

# Shared runner modules live two directories above the vendored suite.
# ruff: noqa: E402

import argparse
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import build_report, validate_pair, write_reports

ID = "test.test_subprocess.POSIXProcessTestCase.test_select_unbuffered"


def verify_case(raw):
    [case] = raw["rows"][ID]
    roles = dict(
        body="cpython", assertions="cpython", operation="mixed", fixture="cpython"
    )
    if (
        raw["rows"].keys() != {ID}
        or case["roles"] != roles
        or case["body_attempt"]["status"] != "blocked"
    ):
        raise ValueError("wrong unchanged method, roles, or native attempt")
    original = case["observed"]
    if original["data_child_argv"][1:] != [
        "-c",
        'import sys;sys.stdout.write("apple")',
    ]:
        raise ValueError("wrong original data child")
    for name, obs in (("original", original), *raw["controls"].items()):
        buffered = name == "buffered-held"
        if not (
            obs["native_read_result"] == (5 if buffered else 4)
            and (
                obs["native_read_request"] > 4
                if buffered
                else obs["native_read_request"] == 4
            )
            and obs["native_returned"] == "appl"
            and obs["unread_kernel_bytes"] == (0 if buffered else 1)
            and obs["host_select_returned_original_stream"] is (not buffered)
            and (name == "original" or obs["data_child_alive_at_select"] is True)
            and obs["native_reaps"] == obs["native_pipe_closes"] == 1
        ):
            raise ValueError(f"missing native pipe read, readiness or cleanup: {name}")
    if raw["controls"].keys() != {"raw-held", "buffered-held"}:
        raise ValueError("need both live-child controls")
    return case


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for opt in ("previous", "readiness", "result"):
        parser.add_argument("--" + opt, type=Path, required=True)
    args = parser.parse_args()
    previous = json.loads(args.previous.read_text())
    data = json.loads(args.readiness.read_text())
    ref = json.loads((HERE / "reference-linux.json").read_text())
    validate_pair(previous, ref)
    result = []
    for prev in previous:
        level = prev["opt_level"]
        [raw] = [r for r in data["reports"] if r["opt_level"] == level]
        if raw["cpython_commit"] != ref["cpython_commit"]:
            raise ValueError("wrong CPython revision")
        case = verify_case(raw)
        rows = prev["records"]
        seen = [(r["id"], r["fixture_variant"]) for r in rows]
        exceptions = {
            (r["id"], r["occurrence"], level): r["exception"]
            for r in rows
            if "exception" in r
        }
        obs = {
            (r["id"], r["occurrence"]): {
                k: r[k] for k in ("outcome", "roles", "reason", "evidence") if k in r
            }
            for r in rows
        }
        obs[ID, 1] = dict(
            outcome="pass",
            roles=case["roles"],
            evidence=dict(
                opt_level=level,
                imre_revision=data["imre_revision"],
                tested_tree=data["tested_tree"],
                source=args.readiness.name,
                operation="Imre launches the original CPython data child, reads exactly four bytes on its raw stdout pipe, closes and reaps. Host select returns that stream with one byte still in the kernel pipe.",
                fixture="CPython runs the original test, assertions and addCleanup. Held-open raw and buffered writers distinguish readable data from EOF.",
                native_body_diagnostic=case["body_attempt"]["reason"],
            ),
        )
        result.append(build_report(ref, level, seen, obs, exceptions=exceptions))
    write_reports(args.result, result, ref)


if __name__ == "__main__":
    main()
