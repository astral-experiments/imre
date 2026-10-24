"""Measure warm calls to the same CRC32C core in three independent processes."""

import argparse
import json
import os
import platform
import statistics
import subprocess
import sys
from pathlib import Path
from time import perf_counter_ns

DRIVER = Path(__file__).resolve().parents[1] / "examples/google_crc32c/driver.py"
CASES = [(8, 2_000_000), (4096, 32_768), (1_048_576, 128)]


def exchange(child, repetitions):
    child.stdin.write(f"{repetitions}\n")
    child.stdin.flush()
    return int(child.stdout.readline())


def benchmark(commands, environment, rounds):
    data = []
    for operation in ("value", "extend"):
        for size, repetitions in CASES:
            workers = {}
            samples = {name: [] for name in commands}
            try:
                for name, command in commands.items():
                    process = subprocess.Popen(
                        [*command, "measure", operation, str(size)],
                        stdin=subprocess.PIPE,
                        stdout=subprocess.PIPE,
                        text=True,
                        env=environment,
                    )
                    workers[name] = process
                    assert process.stdout.readline() == "crc32c-ready\n", name
                checksum = {
                    exchange(process, repetitions) for process in workers.values()
                }
                assert len(checksum) == 1, checksum
                expected = checksum.pop()
                names = list(workers)
                for round_index in range(rounds):
                    offset = round_index % len(names)
                    for name in names[offset:] + names[:offset]:
                        start = perf_counter_ns()
                        observed = exchange(workers[name], repetitions)
                        elapsed = perf_counter_ns() - start
                        assert observed == expected, (
                            name,
                            operation,
                            observed,
                            expected,
                        )
                        samples[name].append(elapsed)
                data.append(
                    {
                        "operation": operation,
                        "bytes": size,
                        "repetitions": repetitions,
                        "result": expected,
                        "elapsed_ns": samples,
                        "median_ns_per_call": {
                            name: statistics.median(elapsed) / repetitions
                            for name, elapsed in samples.items()
                        },
                    }
                )
            finally:
                for worker in workers.values():
                    if worker.poll() is None:
                        worker.stdin.write("0\n")
                        worker.stdin.close()
                    assert worker.wait(timeout=5) == 0
    return data


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cpython-package", required=True, type=Path)
    parser.add_argument("--direct", required=True, type=Path)
    parser.add_argument(
        "--relar", type=Path, help="omit to measure the two baselines only"
    )
    parser.add_argument(
        "--cpu", type=int, help="pin the parent and children to this Linux CPU"
    )
    parser.add_argument("--rounds", type=int, default=9)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if args.cpu is not None:
        os.sched_setaffinity(0, {args.cpu})
    environment = {**os.environ, "PYTHONPATH": str(args.cpython_package.resolve())}
    subprocess.run(
        [
            sys.executable,
            "-B",
            "-c",
            "import google_crc32c; assert google_crc32c.implementation == 'c'",
        ],
        env=environment,
        check=True,
    )
    commands = {
        "direct_c": [args.direct.resolve()],
        "cpython": [sys.executable, "-u", "-B", DRIVER],
    }
    if args.relar:
        commands["relar"] = [args.relar.resolve()]
    data = benchmark(commands, environment, args.rounds)
    document = {
        "platform": platform.platform(),
        "python": platform.python_version(),
        "cpu": args.cpu,
        "rounds": args.rounds,
        "warmup_batches": 1,
        "cases": data,
    }
    args.output.write_text(json.dumps(document, indent=2) + "\n")
    for case in data:
        print(case["operation"], case["bytes"], case["median_ns_per_call"])


if __name__ == "__main__":
    main()
