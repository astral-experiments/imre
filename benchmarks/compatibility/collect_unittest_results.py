"""Capture a generated unittest launcher, including interrupted original IDs."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import signal
import subprocess
from pathlib import Path

PREFIX = "IMRE_UNITTEST "


def parse(stderr: bytes, timed_out: bool) -> dict:
    inventory = []
    occurrences = {}
    fixtures = []
    subtest_skips = []
    count = summary = active = None
    next_start = 0
    lines = stderr.decode("utf-8", errors="replace").splitlines()
    for line_number, line in enumerate(lines, 1):
        if not line.startswith(PREFIX):
            continue
        try:
            event = json.loads(line[len(PREFIX):])
        except json.JSONDecodeError:
            if timed_out and line_number == len(lines):
                break  # The process may have been killed while writing its last event.
            raise
        kind = event["event"]
        if kind == "collection":
            if count is not None:
                raise ValueError("duplicate collection")
            count = event["count"]
        elif kind == "inventory":
            test_id = event["id"]
            occurrences[test_id] = occurrences.get(test_id, 0) + 1
            inventory.append({"id": test_id, "occurrence": occurrences[test_id], "outcome": "not_run"})
        elif kind == "start":
            if active is not None:
                raise ValueError("overlapping tests")
            # Fixtures can prevent some collected tests from ever starting.
            while next_start < len(inventory) and inventory[next_start]["id"] != event["id"]:
                next_start += 1
            if next_start == len(inventory):
                raise ValueError(f"test started outside the inventory: {event['id']}")
            active = inventory[next_start]
            next_start += 1
        elif kind == "result":
            if active is None or active["id"] != event["id"]:
                raise ValueError(f"result without matching start: {event['id']}")
            active.update(outcome=event["outcome"], details=event["details"], line=line_number)
            active = None
        elif kind == "fixture":
            fixtures.append(event)
        elif kind == "subtest_skip":
            subtest_skips.append(event)
        elif kind == "summary":
            if summary is not None:
                raise ValueError("duplicate summary")
            summary = event
        else:
            raise ValueError(f"unknown event: {kind}")
    if count is not None and count != len(inventory) and not timed_out:
        raise ValueError(f"collection count {count} differs from inventory {len(inventory)}")
    if active is not None and timed_out:
        active["outcome"] = "timeout"
    if summary is not None:
        completed = sum(row["outcome"] != "not_run" for row in inventory)
        if active is not None or summary["run"] != completed:
            raise ValueError("summary does not match completed originals")
    return {"collection": count, "inventory_complete": count is not None and count == len(inventory),
            "originals": inventory, "fixtures": fixtures,
            "subtest_skips": subtest_skips, "summary": summary, "timed_out": timed_out}


def capture(argv: list[str], timeout: float, env=None) -> tuple[int | None, bytes, bytes, bool]:
    with subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True, env=env) as proc:
        try:
            stdout, stderr = proc.communicate(timeout=timeout)
            return proc.returncode, stdout, stderr, False
        except subprocess.TimeoutExpired:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            stdout, stderr = proc.communicate()
            return None, stdout, stderr, True


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeout", type=float, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--stdout", type=Path, required=True)
    parser.add_argument("--stderr", type=Path, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command or args.timeout <= 0:
        parser.error("supply a command and a positive timeout")
    code, stdout, stderr, timed_out = capture(command, args.timeout)
    args.stdout.write_bytes(stdout)
    args.stderr.write_bytes(stderr)
    result = parse(stderr, timed_out)
    result.update(argv=command, timeout_seconds=args.timeout, exit_code=code,
                  stdout_sha256=hashlib.sha256(stdout).hexdigest(), stderr_sha256=hashlib.sha256(stderr).hexdigest())
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
