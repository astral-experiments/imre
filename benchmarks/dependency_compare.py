"""Compile optimized Relar once, then compare five planner implementations.

The saved baseline and both Rust references are prebuilt. Ten Williams orders
balance positions and within-round predecessors; the default repeats them three
times. Every fresh-process result must match the complete captured output.
"""

import argparse
import hashlib
import json
import platform
import runpy
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
REFERENCE = HERE / "reference/dependency_planner"
DEPENDENCY_HARNESS = HERE / "dependency_benchmark.py"
dependency = SimpleNamespace(**runpy.run_path(str(DEPENDENCY_HARNESS)))
shared = dependency.shared
validator = dependency.validator
boundaries = SimpleNamespace(**runpy.run_path(str(REFERENCE / "check_boundaries.py")))
ENGINES = ("cpython", "baseline", "relar", "rust_bigint", "rust_i64")
BASE_ORDER = (0, 1, 4, 2, 3)
ORDERS = tuple(
    tuple(ENGINES[(value + shift) % len(ENGINES)] for value in order)
    for shift in range(len(ENGINES))
    for order in (BASE_ORDER, BASE_ORDER[::-1])
)
i64_exclusion = boundaries.i64_exclusion


def rust_sources(root: Path) -> dict:
    paths = [
        root / "Cargo.toml",
        root / "Cargo.lock",
        *sorted((root / "src").rglob("*.rs")),
    ]
    if not any(path.suffix == ".rs" for path in paths):
        raise ValueError("Rust source root has no src/**/*.rs files")
    return {str(path.relative_to(root)): shared.sha256(path) for path in paths}


def verify_rust_record(path: Path, sources: dict, hashes: dict) -> dict:
    record = json.loads(path.read_text())
    if record.get("sources") != sources:
        raise ValueError("Rust build record does not match every Cargo/source file")
    contents = record.get("source_contents", {})
    if set(contents) != set(sources) or any(
        hashlib.sha256(contents[name].encode()).hexdigest() != digest
        for name, digest in sources.items()
    ):
        raise ValueError("Rust build record must preserve every exact source file")
    for name in ("bigint", "i64"):
        if (
            record.get("binaries", {}).get(name, {}).get("sha256")
            != hashes[f"rust_{name}"]
        ):
            raise ValueError(f"Rust build record does not match {name} executable")
    for key in ("rustc_verbose", "cargo_version"):
        if not record.get(key):
            raise ValueError(f"Rust build record must contain {key}")
    script = record.get("build_script", "")
    if hashlib.sha256(script.encode()).hexdigest() != hashes["rust_build_script"]:
        raise ValueError(
            "Rust build script differs; select the original --rust-source-root"
        )
    if record.get("build_script_sha256") != hashes["rust_build_script"]:
        raise ValueError("Rust build-script hash differs from its recorded source")
    return record


def workload_tasks(workload: dict) -> tuple:
    rows = workload["input"].splitlines()
    count = int(rows[0])
    if count != workload["tasks"] or len(rows) != count + 2:
        raise ValueError("captured workload input structure differs from its metadata")
    tasks = []
    for row in rows[1 : count + 1]:
        name, duration, *dependencies = row.split()
        tasks.append(
            validator.TaskSpec(name, int(duration), tuple(map(int, dependencies)))
        )
    if rows[-1] != workload["lookup"]:
        raise ValueError("captured workload lookup differs from its metadata")
    if sum(len(task.dependencies) for task in tasks) != workload["dependencies"]:
        raise ValueError("captured dependency count differs from its metadata")
    return tuple(tasks)


def compare(args: argparse.Namespace) -> dict:
    started = datetime.now(timezone.utc).isoformat()
    cpu = shared.pin_cpu(args.cpu)
    python = shared.executable(args.python)
    source_root = args.rust_source_root.resolve(strict=True)
    artifacts = args.artifact_dir.resolve()
    binary = artifacts / "dependency_planner"
    llvm = artifacts / "dependency_planner.ll"
    inputs = {
        "baseline": Path(shared.executable(str(args.baseline))),
        "baseline_record": args.baseline_record.resolve(strict=True),
        "rust_bigint": Path(shared.executable(str(args.rust_bigint))),
        "rust_i64": Path(shared.executable(str(args.rust_i64))),
        "rust_record": args.rust_build_record.resolve(strict=True),
        "rust_build_script": source_root / "build.py",
        "compiler": Path(shared.executable(str(args.compiler))),
        "runtime": args.runtime.resolve(strict=True),
        "clang": Path(shared.executable(args.clang)),
        "harness": Path(__file__).resolve(),
        "dependency_harness": DEPENDENCY_HARNESS,
        "shared_harness": HERE / "benchmark.py",
        "route_validator": HERE / "validate.py",
        "validator": HERE / "dependency_validate.py",
        "boundary_harness": REFERENCE / "check_boundaries.py",
    }
    source_paths = validator.package_sources()
    sources = {
        str(path.relative_to(validator.PACKAGE_ROOT)): shared.sha256(path)
        for path in source_paths
    }
    rust_hashes = rust_sources(source_root)
    if rust_sources(REFERENCE) != rust_hashes:
        raise ValueError("saved Rust source differs from the tracked reference")
    hashes = {name: shared.sha256(path) for name, path in inputs.items()}
    provenance = dependency.baseline_provenance(
        inputs["baseline_record"], hashes["baseline"], sources
    )
    record = json.loads(inputs["baseline_record"].read_text())
    rust_record = verify_rust_record(inputs["rust_record"], rust_hashes, hashes)
    python_info = json.loads(
        shared.run([python, "-I", "-c", shared.PYTHON_INFO], b"", args.timeout)[
            "stdout"
        ]
    )
    if python_info["implementation"] != "cpython":
        raise ValueError("--python must select CPython")
    python_path = Path(python_info.pop("executable")).resolve()
    inputs["python"] = python_path
    hashes["python"] = shared.sha256(python_path)
    python_info["executable_name"] = python_path.name
    protected = {
        *inputs.values(),
        *source_paths,
        *(source_root / name for name in rust_hashes),
        *(REFERENCE / name for name in rust_hashes),
    }
    boundaries.protect_outputs(
        [binary, llvm, args.output, args.output.with_name(args.output.name + ".tmp")],
        list(protected),
    )
    if len({inputs[name] for name in ("baseline", "rust_bigint", "rust_i64")}) != 3:
        raise ValueError("native executables must have distinct paths")
    captured = {(w["shape"], w["tasks"]): w for w in record.get("workloads", [])}
    required = {(shape, size) for shape in args.shapes for size in args.sizes}
    if not required <= captured.keys():
        raise ValueError("requested workload is absent from the baseline record")
    workloads = [captured[shape, size] for shape in args.shapes for size in args.sizes]
    for work in workloads:
        if hashlib.sha256(work["input"].encode()).hexdigest() != work["input_sha256"]:
            raise ValueError("captured input hash does not match")
        output = work["expected_stdout"].encode()
        if (
            hashlib.sha256(output).hexdigest() != work["expected_stdout_sha256"]
            or len(output) != work["expected_stdout_bytes"]
        ):
            raise ValueError("captured expected-output evidence does not match")
        exclusion = i64_exclusion(workload_tasks(work))
        if exclusion is not None:
            raise ValueError(f"measured workload cannot use checked i64: {exclusion}")
    revision = shared.run(
        ["git", "-C", str(ROOT), "rev-parse", "HEAD"], b"", args.timeout
    )["stdout"].strip()
    dirty = bool(
        shared.run(
            ["git", "-C", str(ROOT), "status", "--porcelain"], b"", args.timeout
        )["stdout"]
    )
    compiler_version = shared.run(
        [str(inputs["compiler"]), "--version"], b"", args.timeout
    )["stdout"].strip()
    clang_version = shared.run([str(inputs["clang"]), "--version"], b"", args.timeout)[
        "stdout"
    ].strip()
    artifacts.mkdir(parents=True, exist_ok=True)
    compilation = shared.run(
        [
            str(inputs["compiler"]),
            "build",
            str(validator.ENTRY),
            "--module-root",
            str(validator.PACKAGE_ROOT),
            "-o",
            str(binary),
            "--runtime",
            str(inputs["runtime"]),
            "--clang",
            str(inputs["clang"]),
            "--opt-level",
            "3",
            "--emit-llvm",
            str(llvm),
        ],
        b"",
        args.timeout,
    )
    inputs.update(binary=binary, llvm_ir=llvm)
    hashes.update({name: shared.sha256(inputs[name]) for name in ("binary", "llvm_ir")})
    commands = {
        "cpython": validator.python_command(str(python_path)),
        "baseline": [str(inputs["baseline"])],
        "relar": [str(binary)],
        "rust_bigint": [str(inputs["rust_bigint"])],
        "rust_i64": [str(inputs["rust_i64"])],
    }
    validation = boundaries.validate_oracle(commands)
    boundary_validation = boundaries.validate(commands, args.timeout)
    measured = []
    for workload in workloads:
        stdin = workload["input"].encode("utf-8")
        expected = workload["expected_stdout"]
        phases = {"warmups": [], "samples": []}
        for phase, count in (("warmups", args.warmups), ("samples", args.samples)):
            for index in range(count):
                order = ORDERS[index % len(ORDERS)]
                sample = {"index": index, "order": order}
                for name in order:
                    result = shared.run(commands[name], stdin, args.timeout)
                    if result["stdout"] != expected:
                        raise AssertionError(
                            f"{name} output mismatch for {workload['shape']}/{workload['tasks']}, {phase} {index}"
                        )
                    sample[name] = dependency.compact_output(result)
                phases[phase].append(sample)
        summaries = {name: shared.summary(phases["samples"], name) for name in ENGINES}
        medians = {
            name: summary["median_seconds"] for name, summary in summaries.items()
        }
        ratios = {
            f"{numerator}_over_{denominator}": medians[numerator] / medians[denominator]
            for numerator, denominator in (
                ("baseline", "relar"),
                ("cpython", "baseline"),
                ("cpython", "relar"),
                ("baseline", "rust_bigint"),
                ("baseline", "rust_i64"),
                ("relar", "rust_bigint"),
                ("relar", "rust_i64"),
                ("rust_bigint", "rust_i64"),
                ("cpython", "rust_bigint"),
                ("cpython", "rust_i64"),
            )
        }
        measured.append(
            {
                **{
                    key: workload[key]
                    for key in (
                        "shape",
                        "tasks",
                        "dependencies",
                        "seed",
                        "lookup",
                        "input",
                        "input_sha256",
                        "expected_stdout",
                        "expected_stdout_sha256",
                        "expected_stdout_bytes",
                    )
                },
                **phases,
                "summary": {**summaries, "ratios_of_medians": ratios},
            }
        )
        print(
            f"{workload['shape']}, {workload['tasks']} tasks: "
            + ", ".join(f"{name} {medians[name] * 1000:.3f}ms" for name in ENGINES),
            flush=True,
        )
    if (
        validator.package_sources() != source_paths
        or rust_sources(source_root) != rust_hashes
        or rust_sources(REFERENCE) != rust_hashes
    ):
        raise RuntimeError("Python or Rust source set changed during the comparison")
    for path in source_paths:
        if (
            shared.sha256(path)
            != sources[str(path.relative_to(validator.PACKAGE_ROOT))]
        ):
            raise RuntimeError("Python source changed during the comparison")
    for name, path in inputs.items():
        if shared.sha256(path) != hashes[name]:
            raise RuntimeError(f"{name} changed during the comparison")
    return {
        "schema_version": 3,
        "example": "dependency_planner",
        "comparison": "baseline and optimized Relar vs unchanged CPython and direct Rust BigInt/checked-i64 references",
        "started_at_utc": started,
        "completed_at_utc": datetime.now(timezone.utc).isoformat(),
        "git": {"revision": revision, "dirty": dirty},
        "sources": sources,
        "rust_sources": rust_hashes,
        "sha256": hashes,
        "artifacts": {
            name: {"bytes": inputs[name].stat().st_size}
            for name in (
                "baseline",
                "binary",
                "llvm_ir",
                "rust_bigint",
                "rust_i64",
            )
        },
        "baseline_provenance": provenance,
        "rust_build_record": rust_record,
        "tools": {
            "relar": compiler_version,
            "clang": clang_version,
            "cpython": python_info,
            "harness_python": sys.version,
        },
        "compilation": compilation,
        "host": {
            "os": platform.system(),
            "kernel_release": platform.release(),
            "architecture": platform.machine(),
            "cpu": cpu,
        },
        "clock": vars(time.get_clock_info("perf_counter")),
        "options": {
            "sizes": args.sizes,
            "shapes": args.shapes,
            "samples": args.samples,
            "warmups": args.warmups,
            "timeout_seconds": args.timeout,
            "validation_timeout_seconds": 120,
        },
        "method": {
            "timer": "time.perf_counter_ns around subprocess.run, using benchmark.py",
            "includes": "fresh process startup, input/output pipes, parsing, complete scheduling/report execution, and process exit; CPython also imports the unchanged package",
            "excludes": "all builds, validation, warmups, output comparisons, and hashes",
            "order_cycle": ORDERS,
            "fully_balanced_samples": args.samples % len(ORDERS) == 0,
            "ordering": "ten Williams orders repeat cyclically; each ten-round cycle places every implementation in every position twice and each directed adjacent pair within a round twice; cross-round predecessors are not balanced",
            "cpu": "harness and all children inherit one CPU affinity",
            "commands": {
                "cpython": [
                    python_path.name,
                    "-I",
                    "-c",
                    validator.BOOTSTRAP,
                    "<package-root>",
                ],
                "baseline": ["<baseline-binary>"],
                "relar": ["<optimized-binary>"],
                "rust_bigint": ["<rust-bigint-binary>"],
                "rust_i64": ["<rust-i64-binary>"],
            },
            "compile_command": [
                inputs["compiler"].name,
                "build",
                "examples/dependency_planner/__main__.py",
                "--module-root",
                "examples",
                "-o",
                "<artifact-dir>/dependency_planner",
                "--runtime",
                "<runtime>",
                "--clang",
                inputs["clang"].name,
                "--opt-level",
                "3",
                "--emit-llvm",
                "<artifact-dir>/dependency_planner.ll",
            ],
            "output": "complete expected stdout retained once per workload; every run must match it exactly and retain status, stderr, duration, output SHA256 and byte length",
            "integer_semantics": "Rust BigInt validates arbitrary-precision durations/dependencies; checked i64 excludes explicitly listed unrepresentable cases and aborts on overflow, so it is a narrower numerical contract",
            "scope": "whole-process dependency-planner comparison; Rust is a direct implementation of the same algorithm, not the same compiled source; no general Python/Rust/C performance claim",
        },
        "validation": validation,
        "boundary_validation": boundary_validation,
        "workloads": measured,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--baseline-record", type=Path, required=True)
    parser.add_argument("--rust-bigint", type=Path, required=True)
    parser.add_argument("--rust-i64", type=Path, required=True)
    parser.add_argument("--rust-build-record", type=Path, required=True)
    parser.add_argument("--rust-source-root", type=Path, default=REFERENCE)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--python", default="python3.14")
    parser.add_argument("--clang", default="clang")
    parser.add_argument(
        "--artifact-dir", type=Path, default=HERE / "local/dependency-planner-optimized"
    )
    parser.add_argument(
        "--sizes", type=shared.positive_int, nargs="+", default=[128, 2048, 16384]
    )
    parser.add_argument(
        "--shapes", choices=("layered", "wide"), nargs="+", default=["layered", "wide"]
    )
    parser.add_argument("--samples", type=shared.positive_int, default=30)
    parser.add_argument("--warmups", type=shared.positive_int, default=2)
    parser.add_argument("--cpu", type=int, help="default: first allowed CPU")
    parser.add_argument("--timeout", type=shared.positive_float, default=120.0)
    parser.add_argument(
        "--output", type=Path, default=HERE / "local/dependency-comparison.json"
    )
    args = parser.parse_args()
    if args.samples % len(ORDERS):
        parser.error("--samples must be a multiple of 10 for balanced execution order")
    result = compare(args)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.output.with_name(args.output.name + ".tmp")
    temporary.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")
    temporary.replace(args.output)
    print(f"Saved {args.output}")


if __name__ == "__main__":
    main()
