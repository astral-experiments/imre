"""Run independent native suites concurrently, retaining a log for each suite."""

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
from contextlib import suppress
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
from typing import TextIO


def run_suite(command: list[str], output: TextIO, kill_failed_children: bool) -> int:
    if not kill_failed_children:
        return subprocess.run(command, stdout=output, stderr=subprocess.STDOUT).returncode

    with subprocess.Popen(command, stdout=output, stderr=subprocess.STDOUT,
                          start_new_session=True) as suite:
        # Keep the suite PID reserved until the signal has reached its process group.
        status = os.waitid(os.P_PID, suite.pid, os.WEXITED | os.WNOWAIT)
        if status.si_status:
            with suppress(ProcessLookupError):
                os.killpg(suite.pid, signal.SIGKILL)
        return suite.wait()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    cpu_count = getattr(os, "process_cpu_count", os.cpu_count)() or 1
    parser.add_argument("--workers", type=int, default=min(4, cpu_count),
                        help="number of suites to run at once (default: available CPUs, up to 4)")
    parser.add_argument("--log-dir", required=True, type=Path)
    parser.add_argument("--kill-failed-children", action="store_true",
                        help="stop any processes left in a failed suite's process group (POSIX)")
    parser.add_argument("--suites", nargs="+", required=True)
    parser.add_argument("arguments", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.workers < 1:
        parser.error("--workers must be positive")
    if args.kill_failed_children and not all(hasattr(os, op) for op in ("waitid", "WNOWAIT")):
        parser.error("--kill-failed-children requires POSIX waitid with WNOWAIT")
    if len(set(args.suites)) != len(args.suites):
        parser.error("suite names must be distinct")
    directory = Path(__file__).resolve().parent
    for suite in args.suites:
        if not suite.isidentifier() or not (directory / f"{suite}.py").is_file():
            parser.error(f"unknown suite: {suite}")
        if suite == "run_suites":
            parser.error("run_suites cannot invoke itself")
    forwarded = args.arguments
    if forwarded[:1] == ["--"]:
        forwarded = forwarded[1:]
    logs = args.log_dir.resolve()
    logs.mkdir(parents=True, exist_ok=False)

    def run(suite: str) -> tuple[str, int, float]:
        started = time.monotonic()
        command = [sys.executable, str(directory / f"{suite}.py"), *forwarded]
        with (logs / f"{suite}.log").open("w", encoding="utf-8") as output:
            try:
                status = run_suite(command, output, args.kill_failed_children)
            except OSError as error:
                output.write(f"Failed to start suite: {error}\n")
                status = 1
        return suite, status, time.monotonic() - started

    failed = []
    print(f"Running {len(args.suites)} suites with {args.workers} workers; logs: {logs}", flush=True)
    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        futures = [pool.submit(run, suite) for suite in args.suites]
        for future in as_completed(futures):
            suite, status, elapsed = future.result()
            print(f"{'FAIL' if status else 'PASS'} {suite} ({elapsed:.1f}s): {logs / (suite + '.log')}", flush=True)
            if status:
                failed.append(suite)
    for suite in sorted(failed):
        print(f"\nLast lines from {suite}:", file=sys.stderr)
        lines = (logs / f"{suite}.log").read_text(encoding="utf-8", errors="replace").splitlines()
        print("\n".join(lines[-30:]), file=sys.stderr)
    print(f"Completed {len(args.suites)} suites; {len(failed)} failed. Logs: {logs}", flush=True)
    raise SystemExit(bool(failed))


if __name__ == "__main__":
    main()
