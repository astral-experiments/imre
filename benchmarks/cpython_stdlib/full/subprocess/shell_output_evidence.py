"""Apply the native shell launches to one original host-driven occurrence."""

# Shared runner modules live two directories above the vendored suite.
# ruff: noqa: E402

import argparse
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import build_report, validate_pair, write_reports

ID = "test.test_subprocess.MiscTests.test_getoutput"


def verify_case(raw):
    [case] = raw["rows"][ID]
    if (
        raw["rows"].keys() != {ID}
        or case["roles"]
        != dict(
            body="cpython", assertions="cpython", fixture="cpython", operation="imre"
        )
        or not case["original_tempdir_removed"]
        or raw["body_attempt"]["status"] != "blocked"
        or raw["withheld_status"]
        != dict(
            native_exit=1,
            original_failure="AssertionError: 0 == 0",
            original_tempdir_removed=True,
        )
    ):
        raise ValueError(
            "wrong method, fixture, native attempt or original negative control"
        )
    ops = case["calls"]
    if [
        (o["api"], o["command"], o["native_exit"], o["child_write_fd"]) for o in ops
    ] != [
        ("getoutput", "echo xyzzy", 0, 1),
        ("getstatusoutput", "echo xyzzy", 0, 1),
        ("getstatusoutput", "cat <original-tempdir>/foo", 1, 2),
    ]:
        raise ValueError(
            "need the original three shell calls, writers and exit statuses"
        )
    if (
        [o["output"] for o in ops[:2]] != ["xyzzy", "xyzzy"]
        or "<original-tempdir>/foo" not in ops[2]["output"]
        or "No such file or directory" not in ops[2]["output"]
        or any(o["native_read_bytes"] != 6 for o in ops[:2])
        or any(
            not (
                o["stderr_duplicated_to_stdout"]
                and o["native_drain_to_eof"]
                and o["native_reaps"] == 1
                and o["native_read_bytes"] == o["child_write_bytes"] > 0
            )
            for o in ops
        )
    ):
        raise ValueError(
            "missing the original output, native pipe drain or successful reap"
        )
    return case


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for opt in ("previous", "shell-output", "result"):
        parser.add_argument("--" + opt, type=Path, required=True)
    args = parser.parse_args()
    previous = json.loads(args.previous.read_text())
    data = json.loads(args.shell_output.read_text())
    ref = json.loads((HERE / "reference-linux.json").read_text())
    validate_pair(previous, ref)
    if data["schema"] != "imre.subprocess.shell-output/v1":
        raise ValueError("unknown shell output report")
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
                source=args.shell_output.name,
                operation="Imre launches all three original shell commands, drains the same pipe written by the shell on stdout or cat on stderr and reaps statuses 0, 0 and 1.",
                fixture="CPython runs the unchanged method, assertions and temporary directory cleanup. Replacing the observed status 1 with zero fails its original assertion.",
                native_body_diagnostic=raw["body_attempt"]["reason"],
            ),
        )
        result.append(build_report(ref, level, seen, obs, exceptions=exceptions))
    write_reports(args.result, result, ref)


if __name__ == "__main__":
    main()
