"""Update the four observed wait/signal occurrences, preserving the full reference."""

import argparse
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from signal_race_originals import IDS as EXTERNAL
from threadsafe_wait_originals import IDS as CONCURRENT
from suite_evidence import build_report, validate_pair, write_reports


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for n in ("previous", "wait-races", "result"):
        p.add_argument("--" + n, required=True, type=Path)
    a = p.parse_args()
    previous = json.loads(a.previous.read_text())
    data = json.loads(a.wait_races.read_text())
    ref = json.loads((HERE / "reference-linux.json").read_text())
    validate_pair(previous, ref)
    result = []
    for prev in previous:
        level = prev["opt_level"]
        [pair] = [r for r in data["reports"] if r["opt_level"] == level]
        races, external = pair["threadsafe"], pair["external"]
        if (
            any(
                r["cpython_commit"] != ref["cpython_commit"] or r["opt_level"] != level
                for r in (races, external)
            )
            or races["toolchain"] != external["toolchain"]
        ):
            raise ValueError("mismatched original or native build")
        if (
            races["rows"].keys() != CONCURRENT
            or external["rows"].keys() != EXTERNAL
            or external["positive_live_child"]
            != dict(kernel_kill="SIGTERM", wait_status=-15)
        ):
            raise ValueError("missing control or occurrence")
        observations, exceptions, discovery = {}, {}, []
        for row in prev["records"]:
            name, occ = key = row["id"], row["occurrence"]
            discovery.append((name, row["fixture_variant"]))
            if "exception" in row:
                exceptions[name, occ, level] = row["exception"]
            observations[key] = {
                k: row[k]
                for k in ("outcome", "roles", "reason", "evidence")
                if k in row
            }
            if name not in CONCURRENT | EXTERNAL:
                continue
            [case] = (races if name in CONCURRENT else external)["rows"][name]
            roles = dict(
                body="cpython", assertions="cpython", operation="imre", fixture="mixed"
            )
            if (
                row["reference"] != "pass"
                or occ != 1
                or case["roles"] != roles
                or case["body_attempt"]["status"] != "blocked"
            ):
                raise ValueError((name, case))
            evidence = dict(
                opt_level=level,
                imre_revision=data["imre_revision"],
                tested_tree=data["tested_tree"],
                source="wait-races-linux.json",
                native_body_diagnostic=case["body_attempt"]["reason"],
            )
            if name in CONCURRENT:
                op = case["observed"]
                if (
                    not op["owner_timed_wait_overlaps_native_timer"]
                    or op["successful_reaps"] != 1
                    or op["owner_timed_wait"] != -9
                    or op["signal"] != "SIGKILL"
                    or op["timer_operations"]
                    != [
                        ["poll", None],
                        ["kill", 0],
                        ["wait", -9],
                        ["status", -9],
                        ["wait", -9],
                        ["status", -9],
                    ]
                ):
                    raise ValueError((name, op))
                evidence.update(
                    operation="compiled Popen.wait(timeout) overlaps a second native thread's poll, SIGKILL and waits on the same child; one successful reap and stable returncode -9",
                    fixture="unchanged CPython body, assertions, class hooks and Timer; native pthread transports the Timer calls to the same functions emitted for Popen",
                )
                observations[key] = dict(outcome="pass", roles=roles, evidence=evidence)
            else:
                if (
                    not case["external_wait_in_native_parent"]
                    or not case["original_support_wait_process"]
                    or not case["public_wait_observed_ECHILD"]
                    or case["native_signals_sent"] != 0
                    or case["native_child_exit"] != 0
                ):
                    raise ValueError((name, case))
                evidence.update(
                    fixture="unchanged CPython body, assertions and original support.wait_process; native fixture performs its raw waitpid in the actual kernel parent"
                )
                if name.endswith(".test_stopped"):
                    if case["outcome"] != "failure" or case["failure"] != "0 != -3":
                        raise ValueError((name, case))
                    evidence.update(
                        operation="native Popen.wait sees ECHILD after the external reap and returns 0; the host mock injects a synthetic stopped status into CPython's os.waitpid only"
                    )
                    observations[key] = dict(
                        outcome="failure",
                        roles=roles,
                        reason="The original host waitpid mock cannot inject W_STOPCODE(3) into native Popen.wait; the original assertion observes 0 != -3.",
                        evidence=evidence,
                    )
                else:
                    if case["outcome"] != "pass" or case["failure"] is not None:
                        raise ValueError((name, case))
                    evidence.update(
                        operation="native Popen.send_signal observes ECHILD after the external reap; no kill syscall. The separate live-child control sends SIGTERM and returns -15"
                    )
                    observations[key] = dict(
                        outcome="pass", roles=roles, evidence=evidence
                    )
        result.append(
            build_report(ref, level, discovery, observations, exceptions=exceptions)
        )
    write_reports(a.result, result, ref)
    for r in result:
        print(r["opt_level"], r["denominator"], r["counts"])


if __name__ == "__main__":
    main()
