"""Compare prebuilt baseline and optimized Relar executables with unchanged CPython.

This measures fresh-process execution, including startup and pipes. It never
builds either native executable. Twelve measured triplets cover every execution
order twice; use a multiple of six samples to retain complete counterbalancing.
"""

import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import platform
import runpy
import sys
import time
from types import SimpleNamespace


# Reuse the same timing boundaries, checks, metadata, and validator even when
# PYTHONSAFEPATH prevents ordinary imports from the script's directory.
SHARED_PATH = Path(__file__).resolve().with_name("benchmark.py")
shared = SimpleNamespace(**runpy.run_path(str(SHARED_PATH)))
ORDERS = (
    ("baseline", "optimized", "cpython"),
    ("cpython", "optimized", "baseline"),
    ("optimized", "cpython", "baseline"),
    ("baseline", "cpython", "optimized"),
    ("cpython", "baseline", "optimized"),
    ("optimized", "baseline", "cpython"),
)


def provenance(path: Path, binary_hash: str, source_hash: str) -> dict:
    """Accept a benchmark.py result only if it identifies these exact inputs."""
    record = json.loads(path.read_text())
    recorded_hashes = record.get("sha256", {})
    if recorded_hashes.get("binary") != binary_hash:
        raise ValueError(f"{path.name}: native executable hash does not match")
    if recorded_hashes.get("source") != source_hash:
        raise ValueError(f"{path.name}: source hash does not match the unchanged example")
    return {
        "record_name": path.name,
        "git": record.get("git"),
        "sha256": recorded_hashes,
        "tools": record.get("tools"),
        "compilation": record.get("compilation"),
        "compile_command": record.get("method", {}).get("compile_command"),
    }


def compare(args: argparse.Namespace) -> dict:
    started_at = datetime.now(timezone.utc).isoformat()
    cpu = shared.pin_cpu(args.cpu)
    python = shared.executable(args.python)
    binaries = {
        "baseline": Path(shared.executable(str(args.baseline))),
        "optimized": Path(shared.executable(str(args.optimized))),
    }
    if binaries["baseline"] == binaries["optimized"]:
        raise ValueError("baseline and optimized executables must have distinct paths")
    inputs = {
        **binaries,
        "source": shared.SOURCE,
        "comparison_harness": Path(__file__).resolve(),
        "benchmark_harness": SHARED_PATH,
        "validator": shared.ROOT / "benchmarks" / "validate.py",
    }
    for name in binaries:
        record = getattr(args, f"{name}_record")
        if record is not None:
            inputs[f"{name}_record"] = record.resolve(strict=True)
    hashes = {name: shared.sha256(path) for name, path in inputs.items()}
    python_info = json.loads(
        shared.run([python, "-I", "-c", shared.PYTHON_INFO], b"", args.timeout)["stdout"]
    )
    if python_info["implementation"] != "cpython":
        raise ValueError("--python must select CPython")
    python_path = Path(python_info.pop("executable")).resolve()
    inputs["python"] = python_path
    hashes["python"] = shared.sha256(python_path)
    python_info["executable_name"] = python_path.name
    if args.output.resolve() in inputs.values():
        raise ValueError("JSON output must not overwrite a benchmark input")
    records = {
        name: provenance(inputs[f"{name}_record"], hashes[name], hashes["source"])
        if f"{name}_record" in inputs else None
        for name in binaries
    }
    revision = shared.run(
        ["git", "-C", str(shared.ROOT), "rev-parse", "HEAD"], b"", args.timeout
    )["stdout"].strip()
    dirty = bool(shared.run(
        ["git", "-C", str(shared.ROOT), "status", "--porcelain"], b"", args.timeout
    )["stdout"])
    validation = {
        name: shared.validate(binary, python, shared.SOURCE)
        for name, binary in binaries.items()
    }
    commands = {
        **{name: [str(binary)] for name, binary in binaries.items()},
        "cpython": [python, "-I", str(shared.SOURCE)],
    }
    workloads = []
    for width in args.sizes:
        stdin = shared.input_text(width, args.routes, args.seed).encode("ascii")
        expected = shared.run(commands["cpython"], stdin, args.timeout)["stdout"]
        phases = {"warmups": [], "samples": []}
        for phase, count in (("warmups", args.warmups), ("samples", args.samples)):
            for index in range(count):
                order = ORDERS[index % len(ORDERS)]
                triplet = {"index": index, "order": order}
                for implementation in order:
                    result = shared.run(commands[implementation], stdin, args.timeout)
                    if result["stdout"] != expected:
                        raise AssertionError(
                            f"output mismatch for {implementation}, "
                            f"width {width}, {phase} {index}"
                        )
                    triplet[implementation] = result
                phases[phase].append(triplet)
        summaries = {
            name: shared.summary(phases["samples"], name) for name in commands
        }
        medians = {name: result["median_seconds"] for name, result in summaries.items()}
        speedups = {
            "optimized_vs_baseline": medians["baseline"] / medians["optimized"],
            "baseline_vs_cpython": medians["cpython"] / medians["baseline"],
            "optimized_vs_cpython": medians["cpython"] / medians["optimized"],
        }
        workloads.append({
            "width": width,
            "routes": args.routes,
            "seed": args.seed,
            "input": stdin.decode("ascii"),
            "input_sha256": hashlib.sha256(stdin).hexdigest(),
            "expected_stdout": expected,
            **phases,
            "summary": {**summaries, "speedup_ratio_of_medians": speedups},
        })
        print(
            f"{width}x{width}, {args.routes} routes: "
            f"baseline {medians['baseline']:.6f}s, optimized {medians['optimized']:.6f}s, "
            f"CPython {medians['cpython']:.6f}s; "
            f"optimized {speedups['optimized_vs_baseline']:.2f}x vs baseline, "
            f"{speedups['optimized_vs_cpython']:.2f}x vs CPython",
            flush=True,
        )
    for name, path in inputs.items():
        if shared.sha256(path) != hashes[name]:
            raise RuntimeError(
                f"{name} changed during the benchmark; discard these measurements"
            )
    return {
        "schema_version": 1,
        "comparison": "prebuilt baseline vs optimized Relar vs unchanged CPython",
        "started_at_utc": started_at,
        "completed_at_utc": datetime.now(timezone.utc).isoformat(),
        "git": {"revision": revision, "dirty": dirty},
        "sha256": hashes,
        "tools": {"cpython": python_info},
        "native_build_records": records,
        "host": {
            "os": platform.system(),
            "kernel_release": platform.release(),
            "architecture": platform.machine(),
            "cpu": cpu,
        },
        "clock": vars(time.get_clock_info("perf_counter")),
        "artifacts": {
            name: {"executable_bytes": path.stat().st_size}
            for name, path in binaries.items()
        },
        "options": {
            "sizes": args.sizes,
            "routes": args.routes,
            "seed": args.seed,
            "samples": args.samples,
            "warmups": args.warmups,
            "timeout_seconds": args.timeout,
        },
        "method": {
            "timer": "time.perf_counter_ns around subprocess.run; shared with benchmark.py",
            "includes": "fresh process startup, stdin/stdout pipes, complete route-planner execution, and process exit",
            "excludes": "all native compilation, correctness validation, and warmups",
            "ordering": "all six orders in a repeating cycle; adjacent orders reverse each other",
            "order_cycle": ORDERS,
            "fully_counterbalanced_samples": args.samples % len(ORDERS) == 0,
            "cpu": "harness and all child processes inherit the same single-CPU affinity",
            "commands": {
                "baseline": ["<baseline-executable>"],
                "optimized": ["<optimized-executable>"],
                "cpython": [python_path.name, "-I", "examples/route_planner.py"],
            },
            "native_provenance": "optional build records verify source and executable hashes; toolchains are not inferred from prebuilt binaries",
            "spread": "minimum, maximum and quartiles describe observed samples, not confidence intervals",
        },
        "validation": validation,
        "workloads": workloads,
    }


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__,
        epilog="Optional build records are JSON outputs from benchmark.py. The validator retains its own 120-second subprocess timeout.",
    )
    parser.add_argument(
        "--baseline", type=Path,
        default=shared.LOCAL / "profile-baseline" / "route_planner",
    )
    parser.add_argument("--optimized", type=Path, required=True)
    parser.add_argument("--baseline-record", type=Path)
    parser.add_argument("--optimized-record", type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--output", type=Path, default=shared.LOCAL / "comparison.json")
    parser.add_argument("--sizes", type=shared.positive_int, nargs="+", default=[32, 128, 256])
    parser.add_argument("--routes", type=shared.positive_int, default=3)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--samples", type=shared.positive_int, default=12)
    parser.add_argument("--warmups", type=shared.positive_int, default=2)
    parser.add_argument("--cpu", type=int)
    parser.add_argument("--timeout", type=shared.positive_float, default=120.0)
    args = parser.parse_args()
    result = compare(args)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.output.with_name(args.output.name + ".tmp")
    temporary.write_text(json.dumps(result, indent=2) + "\n")
    temporary.replace(args.output)
    print(f"Saved {args.output}")


if __name__ == "__main__":
    main()
