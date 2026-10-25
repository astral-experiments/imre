"""Measure edits and reads of repeated vs already packed struct lists (Linux)."""

import argparse
import hashlib
import json
import math
import os
import platform
import shutil
import statistics
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "benchmarks/fixtures/struct_edit_density.py"
FIELDS = ("build_ms", "edits_ms", "last_edit_ms", "scans_ms", "probes_ms")
LAYOUTS = ("adaptive", "packed")
PATTERNS = {"clustered": 1, "scattered": 65_537}


def digest(path):
    with open(path, "rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def checksums(n, k, stride, passes, probes):
    # Count changed slots seen by the probe via the modular inverse. This oracle
    # neither builds a list nor reads any rows from the measured program.
    inv = pow(stride, -1, n)
    hits = sum((((i * 100_003 + 7) % n - n // 2) * inv) % n < k for i in range(probes))
    return (25 * n + 75 * k) * passes * (passes + 1) // 2, 25 * probes + 75 * hits


def sample(cmd, args, expected, n):
    with tempfile.NamedTemporaryFile(mode="r") as mem:
        proc = subprocess.run(
            ["/usr/bin/time", "-f", "%M", "-o", mem.name, *cmd, *args],
            capture_output=True,
            text=True,
            check=True,
            timeout=180,
        )
        if proc.stderr:
            raise RuntimeError(proc.stderr)
        length, scan, probe, *times = proc.stdout.splitlines()
        if int(length) != n or (float(scan), float(probe)) != expected:
            raise ValueError(f"unexpected checksums: {proc.stdout}")
        return dict(
            zip(FIELDS, (float(t) * 1000 for t in times), strict=True),
            peak_kib=int(mem.read()),
        )


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--compiler", required=True, type=Path)
    p.add_argument("--runtime", required=True, type=Path)
    p.add_argument("--clang", default="clang-18")
    p.add_argument("--cpu", required=True, type=int)
    p.add_argument("--size", type=int, default=1_000_000)
    p.add_argument(
        "--counts",
        nargs="+",
        type=int,
        default=[0, 1, 1000, 10_000, 62_500, 125_000, 250_000],
    )
    p.add_argument("--rounds", type=int, default=8)
    p.add_argument("--passes", type=int, default=12)
    p.add_argument("--probes", type=int, default=65_536)
    p.add_argument("--output", required=True, type=Path)
    args = p.parse_args()
    if (
        args.size <= 1
        or min(args.counts) < 0
        or max(args.counts) > args.size
        or args.rounds <= 0
        or args.rounds % 2
        or min(args.passes, args.probes) <= 0
        or any(math.gcd(s, args.size) != 1 for s in PATTERNS.values())
    ):
        p.error("invalid size/count; use an even number of rounds and coprime strides")
    if any(
        s.startswith("TracerPid:") and int(s.split()[1])
        for s in Path("/proc/self/status").read_text().splitlines()
    ):
        p.error("run outside ptrace for timing and RSS")
    clang = shutil.which(args.clang)
    data = {
        "size": args.size,
        "passes": args.passes,
        "probes": args.probes,
        "rounds": args.rounds,
        "cpu": args.cpu,
        "machine": platform.machine(),
        "kernel": platform.release(),
        "cpu_model": next(
            s.split(":", 1)[1].strip()
            for s in Path("/proc/cpuinfo").read_text().splitlines()
            if s.startswith("model name")
        ),
        "clang": subprocess.check_output([clang, "--version"], text=True).splitlines()[
            0
        ],
        "fixture_sha256": digest(SOURCE),
        "compiler_sha256": digest(args.compiler),
        "runtime_sha256": digest(args.runtime),
        "workloads": {},
    }
    os.sched_setaffinity(0, {args.cpu})
    with tempfile.TemporaryDirectory(prefix="imre-struct-edits-") as tmp:
        binary = Path(tmp) / "edits"
        llvm = binary.with_suffix(".ll")
        flags = ["--opt-level", "3", "--whole-program", "--no-cache"]
        subprocess.run(
            [
                args.compiler,
                "build",
                SOURCE,
                *flags,
                "--clang",
                clang,
                "--runtime",
                args.runtime,
                "--emit-llvm",
                llvm,
                "-o",
                binary,
            ],
            check=True,
        )
        data.update(flags=flags, binary_sha256=digest(binary), llvm_sha256=digest(llvm))
        for pattern, stride in PATTERNS.items():
            for k in args.counts:
                if pattern == "scattered" and k <= 1:
                    continue  # The two schedules are identical for zero or one change.
                exp = checksums(args.size, k, stride, args.passes, args.probes)
                if max(exp) >= 2**53:
                    raise ValueError("checksum is not an exact binary64 integer")
                base = [str(args.size), str(k), str(stride)]
                timings = {name: [] for name in LAYOUTS}
                for r in range(-1, args.rounds):
                    for name in LAYOUTS if r % 2 == 0 else reversed(LAYOUTS):
                        got = sample(
                            [binary],
                            [*base, name, str(args.passes), str(args.probes)],
                            exp,
                            args.size,
                        )
                        if r >= 0:
                            timings[name].append(got)
                medians = {
                    name: {
                        f: statistics.median(s[f] for s in samples) for f in samples[0]
                    }
                    for name, samples in timings.items()
                }
                data["workloads"][f"{pattern}-{k}"] = {
                    "changes": k,
                    "stride": stride,
                    "checksums": exp,
                    "samples": timings,
                    "medians": medians,
                }
                print(pattern, k, medians, flush=True)
    args.output.write_text(json.dumps(data, indent=2) + "\n")


if __name__ == "__main__":
    main()
