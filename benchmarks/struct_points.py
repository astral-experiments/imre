"""Compare packed structs and frozen dataclasses with identical field and loop code (Linux)."""

import argparse
import hashlib
import json
import os
import platform
import shutil
import statistics
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "benchmarks/fixtures/struct_points.py"
ENGINES = ("Imre struct", "Imre dataclass", "CPython struct", "CPython dataclass")


def digest(path):
    with open(path, "rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def expected(n, passes, layout):
    cycle = [i * i + (i + 1) * (i + 1) for i in range(1024)]
    blocks, tail = divmod(n, len(cycle))
    total = blocks * sum(cycle) + sum(cycle[:tail]) if layout == "distinct" else n * 25
    if layout == "replace":
        total += 100 - 25
    total *= passes * (passes + 1) // 2
    if total >= 2**53:
        raise ValueError("choose sizes and passes with an exact binary64 checksum")
    return total


def run(command, n, passes, layout, oracle, env):
    # GNU time forks the measured process from a small native executable.
    # A direct Python wait4 can report the harness's pre-exec RSS for small programs.
    with tempfile.NamedTemporaryFile(mode="r") as memory:
        proc = subprocess.run(
            [
                "/usr/bin/time",
                "-f",
                "%M",
                "-o",
                memory.name,
                *command,
                str(n),
                str(passes),
                layout,
            ],
            text=True,
            capture_output=True,
            check=True,
            env=env,
            timeout=120,
        )
        if proc.stderr:
            raise RuntimeError(proc.stderr)
        peak_kib = int(memory.read())
        length, checksum, build, write, scan = proc.stdout.splitlines()
    if int(length) != n or float(checksum) != oracle:
        raise ValueError(
            f"incorrect result: {length}, {checksum}, expected {n}, {oracle}"
        )
    return {
        "build_ms": 1000 * float(build),
        "first_write_ms": 1000 * float(write),
        "scan_ms": 1000 * float(scan),
        "peak_kib": peak_kib,
    }


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--compiler", type=Path, required=True)
    p.add_argument("--runtime", type=Path, required=True)
    p.add_argument("--clang", default="clang-18")
    p.add_argument("--python", default="python3.14")
    p.add_argument("--cpu", type=int, required=True)
    p.add_argument("--sizes", type=int, nargs="+", default=[10_000, 1_000_000])
    p.add_argument("--passes", type=int, default=12)
    p.add_argument("--rounds", type=int, default=8)
    p.add_argument("--output", type=Path, required=True)
    args = p.parse_args()
    if args.rounds <= 0 or args.rounds % 4 or min(args.sizes) <= 0 or args.passes <= 0:
        p.error("use positive sizes/passes and a positive multiple of four rounds")
    if int(
        next(
            s.split()[1]
            for s in Path("/proc/self/status").read_text().splitlines()
            if s.startswith("TracerPid:")
        )
    ):
        p.error("run outside ptrace for comparable timing and RSS")
    compiler, runtime = args.compiler.resolve(), args.runtime.resolve()
    python = shutil.which(args.python)
    clang = shutil.which(args.clang)
    env = dict(
        os.environ,
        PYTHONPATH=str(ROOT / "stdlib"),
        PYTHONDONTWRITEBYTECODE="1",
        PYTHONHASHSEED="0",
    )
    build_flags = [
        "--clang",
        clang,
        "--opt-level",
        "3",
        "--whole-program",
        "--no-cache",
    ]
    data = {
        "cpu": args.cpu,
        "host": platform.machine(),
        "system": platform.system(),
        "kernel": platform.release(),
        "fixture_sha256": digest(SOURCE),
        "shim_sha256": digest(ROOT / "stdlib/imre/__init__.py"),
        "cpu_model": next(
            s.split(":", 1)[1].strip()
            for s in Path("/proc/cpuinfo").read_text().splitlines()
            if s.startswith("model name")
        ),
        "python": subprocess.check_output([python, "-V"], text=True).strip(),
        "clang": subprocess.check_output([clang, "--version"], text=True).splitlines()[
            0
        ],
        "compiler_sha256": digest(compiler),
        "runtime_sha256": digest(runtime),
        "flags": build_flags,
        "passes": args.passes,
        "rounds": args.rounds,
        "workloads": {},
        "sources": {},
    }
    with tempfile.TemporaryDirectory(prefix="imre-struct-benchmark-") as tmp:
        root = Path(tmp)
        commands = {}
        src = SOURCE.read_text()
        for variant in ("struct", "dataclass"):
            text = (
                src
                if variant == "struct"
                else src.replace(
                    "from imre import struct", "from dataclasses import dataclass"
                ).replace("@struct\n", "@dataclass(frozen=True, slots=True)\n")
            )
            path = root / (variant + "_points.py")
            path.write_text(text)
            binary = root / variant
            llvm = binary.with_suffix(".ll")
            subprocess.run(
                [
                    compiler,
                    "build",
                    path,
                    *build_flags,
                    "--runtime",
                    runtime,
                    "--emit-llvm",
                    llvm,
                    "-o",
                    binary,
                ],
                check=True,
                env=env,
            )
            data["sources"][variant] = {
                "source_sha256": digest(path),
                "llvm_sha256": digest(llvm),
                "binary_sha256": digest(binary),
            }
            commands["Imre " + variant] = [binary]
            commands["CPython " + variant] = [python, path]

        os.sched_setaffinity(0, {args.cpu})
        # The replacement workload measures the first write that changes a repeated value.
        workloads = [(n, "distinct") for n in args.sizes] + [
            (max(args.sizes), "shared"),
            (max(args.sizes), "replace"),
        ]
        for n, layout in workloads:
            oracle = expected(n, args.passes, layout)
            for engine in ENGINES:
                run(commands[engine], n, args.passes, layout, oracle, env)
            samples = {name: [] for name in ENGINES}
            orders = []
            for r in range(args.rounds):
                order = [ENGINES[(i + r) % 4] for i in (0, 1, 3, 2)]
                orders.append(order)
                for engine in order:
                    samples[engine].append(
                        run(commands[engine], n, args.passes, layout, oracle, env)
                    )
            medians = {
                engine: {
                    key: statistics.median(v[key] for v in values) for key in values[0]
                }
                for engine, values in samples.items()
            }
            data["workloads"][f"{layout}-{n}"] = {
                "checksum": oracle,
                "orders": orders,
                "samples": samples,
                "medians": medians,
            }
            print(layout, n, medians, flush=True)
    args.output.write_text(json.dumps(data, indent=2) + "\n")


if __name__ == "__main__":
    main()
