"""Overlay verified kernel capacity observations without upgrading the host assertions."""

import argparse
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from pipe_capacity_originals import IDS, STREAMS
from suite_evidence import build_report, validate_pair, write_reports


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("previous", "pipe-capacities", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    previous = json.loads(args.previous.read_text())
    raw = json.loads(args.pipe_capacities.read_text())
    ref = json.loads((HERE / "reference-linux.json").read_text())
    validate_pair(previous, ref)
    result = []
    for prev in previous:
        level = prev["opt_level"]
        [report] = [r for r in raw["reports"] if r["opt_level"] == level]
        if (
            report["cpython_commit"] != ref["cpython_commit"]
            or report["rows"].keys() != IDS
        ):
            raise ValueError("wrong test revision or missing fixture variant")
        discovery, observations, exceptions = [], {}, {}
        for row in prev["records"]:
            name, occ = key = row["id"], row["occurrence"]
            discovery.append((name, row["fixture_variant"]))
            observations[key] = {
                k: row[k]
                for k in ("outcome", "roles", "reason", "evidence")
                if k in row
            }
            if "exception" in row:
                exceptions[name, occ, level] = row["exception"]
            if name not in IDS:
                continue
            [case] = report["rows"][name]
            op = case["observed"]
            default = name.endswith(".test_pipesize_default")
            [base] = set(op["reference_pipe_capacities"])
            expected = base if default else base // 2
            roles = dict(
                body="cpython", assertions="cpython", operation="imre", fixture="mixed"
            )
            if (
                row["reference"] != "pass"
                or occ != 1
                or case["roles"] != roles
                or case["body_attempt"]["status"] != "blocked"
                or op["requested"] != (-1 if default else expected)
                or op["native_pipe_capacities"] != dict.fromkeys(STREAMS, expected)
                or op["set_size_before_spawn"] != (0 if default else 3)
                or op["distinct_pipe_inodes"] != 3
                or op["successful_reaps"] != 1
                or op["native_parent_selector_calls"] != 0
                or op["wrote_pear"] == default
                or not op["all_native_streams_closed"]
                or op["selector_fixture"]
                != ("SelectSelector" if "NoPoll." in name else "PollSelector")
                or op["kill_observation"]
                not in ("SIGKILL delivered", "already exited and reaped by native poll")
            ):
                raise ValueError((name, op))
            observations[key] = dict(
                outcome="pass",
                roles=roles,
                evidence=dict(
                    opt_level=level,
                    imre_revision=raw["imre_revision"],
                    tested_tree=raw["tested_tree"],
                    source="pipe-capacities-linux.json",
                    native_body_diagnostic=case["body_attempt"]["reason"],
                    operation=f"Imre Popen created three distinct kernel pipes at {expected} bytes each; {op['set_size_before_spawn']} F_SETPIPE_SZ calls before spawn, closed all streams, and reaped the original data child once",
                    fixture="Unchanged CPython class setup, decorated body and assertions; host F_GETPIPE_SZ on the same kernel pipe inodes. The native parent handled the original Popen, stream I/O, kill, and wait.",
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
