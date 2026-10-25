"""Benchmark the unchanged dependency-planner package on CPython and Imre."""

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
shared = SimpleNamespace(**runpy.run_path(str(HERE / "benchmark.py")))
validator = SimpleNamespace(**runpy.run_path(str(HERE / "dependency_validate.py")))
PAIR_ORDERS = (("cpython", "imre"), ("imre", "cpython"))
TRIPLET_ORDERS = (
    ("cpython", "baseline", "imre"),
    ("imre", "baseline", "cpython"),
    ("baseline", "imre", "cpython"),
    ("cpython", "imre", "baseline"),
    ("imre", "cpython", "baseline"),
    ("baseline", "cpython", "imre"),
)


def compact_output(result: dict) -> dict:
    """Retain output evidence after the caller checks the complete output."""
    stdout = result["stdout"].encode("utf-8")
    return {
        **{key: value for key, value in result.items() if key != "stdout"},
        "stdout_sha256": hashlib.sha256(stdout).hexdigest(),
        "stdout_bytes": len(stdout),
    }


def baseline_provenance(path: Path, binary_hash: str, sources: dict) -> dict:
    """Require a build record for this exact native binary and unchanged package."""
    record = json.loads(path.read_text())
    if record.get("example") != "dependency_planner":
        raise ValueError(f"{path.name}: not a dependency-planner benchmark record")
    hashes = record.get("sha256", {})
    if hashes.get("binary") != binary_hash:
        raise ValueError(f"{path.name}: baseline executable hash does not match")
    if record.get("sources") != sources:
        raise ValueError(f"{path.name}: package source hashes do not match")
    return {
        "record_name": path.name,
        "record_sha256": shared.sha256(path),
        "git": record.get("git"),
        "sources": record["sources"],
        "sha256": hashes,
        "tools": record.get("tools"),
        "compilation": record.get("compilation"),
        "compile_command": record.get("method", {}).get("compile_command"),
    }


def benchmark(args: argparse.Namespace) -> dict:
    cpu = shared.pin_cpu(args.cpu)
    compiler = shared.executable(args.compiler)
    python = shared.executable(args.python)
    clang = shared.executable(args.clang)
    runtime = args.runtime.resolve(strict=True)
    artifact_dir = args.artifact_dir.resolve()
    artifact_dir.mkdir(parents=True, exist_ok=True)
    binary = artifact_dir / "dependency_planner"
    llvm = artifact_dir / "dependency_planner.ll"
    inputs = {
        "compiler": Path(compiler),
        "runtime": runtime,
        "clang": Path(clang),
        "python": Path(python),
        "harness": Path(__file__).resolve(),
        "shared_harness": HERE / "benchmark.py",
        "validator": HERE / "dependency_validate.py",
    }
    if args.baseline is not None:
        inputs["baseline"] = Path(shared.executable(str(args.baseline)))
        inputs["baseline_record"] = args.baseline_record.resolve(strict=True)
    source_paths = validator.package_sources()
    source_hashes = {
        str(path.relative_to(validator.PACKAGE_ROOT)): shared.sha256(path)
        for path in source_paths
    }
    if any(
        path in (binary, llvm, args.output.resolve())
        for path in [*inputs.values(), *source_paths]
    ):
        raise ValueError("artifact and JSON paths must not overwrite sources or tools")
    if args.output.resolve() in (binary, llvm):
        raise ValueError("JSON, executable, and LLVM output must have distinct paths")
    hashes = {name: shared.sha256(path) for name, path in inputs.items()}
    provenance = (
        baseline_provenance(
            inputs["baseline_record"], hashes["baseline"], source_hashes
        )
        if args.baseline is not None
        else None
    )
    python_info = json.loads(
        shared.run([python, "-I", "-c", shared.PYTHON_INFO], b"", args.timeout)[
            "stdout"
        ]
    )
    if python_info["implementation"] != "cpython":
        raise ValueError("--python must select CPython")
    python_path = Path(python_info.pop("executable")).resolve()
    if python_path in (binary, llvm, args.output.resolve()):
        raise ValueError("artifact and JSON paths must not overwrite CPython")
    inputs["python"] = python_path
    hashes["python"] = shared.sha256(python_path)
    python_info["executable_name"] = python_path.name
    compiler_version = shared.run([compiler, "--version"], b"", args.timeout)[
        "stdout"
    ].strip()
    clang_version = shared.run([clang, "--version"], b"", args.timeout)[
        "stdout"
    ].strip()
    revision = shared.run(
        ["git", "-C", str(shared.ROOT), "rev-parse", "HEAD"], b"", args.timeout
    )["stdout"].strip()
    dirty = bool(
        shared.run(
            ["git", "-C", str(shared.ROOT), "status", "--porcelain"], b"", args.timeout
        )["stdout"]
    )
    compilation = shared.run(
        [
            compiler,
            "build",
            str(validator.ENTRY),
            "--module-root",
            str(validator.PACKAGE_ROOT),
            "-o",
            str(binary),
            "--runtime",
            str(runtime),
            "--clang",
            clang,
            "--opt-level",
            "3",
            "--emit-llvm",
            str(llvm),
        ],
        b"",
        args.timeout,
    )
    inputs.update({"binary": binary, "llvm_ir": llvm})
    hashes.update({name: shared.sha256(inputs[name]) for name in ("binary", "llvm_ir")})
    validation = validator.validate(binary, python)
    commands = {"cpython": validator.python_command(python), "imre": [str(binary)]}
    baseline_validation = None
    if provenance is not None:
        baseline_validation = validator.validate(inputs["baseline"], python)
        commands["baseline"] = [str(inputs["baseline"])]
    orders = TRIPLET_ORDERS if provenance is not None else PAIR_ORDERS
    workloads = []
    for shape in args.shapes:
        for size in args.sizes:
            tasks = validator.make_workload(size, args.seed, shape)
            lookup = tasks[size // 2].name
            stdin = validator.input_text(tasks, lookup).encode("utf-8")
            expected = validator.expected_output(tasks, lookup)
            reference = shared.run(commands["cpython"], stdin, args.timeout)
            if reference["stdout"] != expected:
                raise AssertionError(
                    f"CPython/oracle mismatch for {shape}, {size} tasks"
                )
            phases = {"warmups": [], "samples": []}
            for phase, count in (("warmups", args.warmups), ("samples", args.samples)):
                for index in range(count):
                    order = orders[index % len(orders)]
                    group = {"index": index, "order": order}
                    for implementation in order:
                        result = shared.run(
                            commands[implementation], stdin, args.timeout
                        )
                        if result["stdout"] != expected:
                            raise AssertionError(
                                f"output mismatch for {implementation}, {shape}, "
                                f"{size} tasks, {phase} {index}"
                            )
                        group[implementation] = compact_output(result)
                    phases[phase].append(group)
            cpython = shared.summary(phases["samples"], "cpython")
            imre = shared.summary(phases["samples"], "imre")
            speedup = cpython["median_seconds"] / imre["median_seconds"]
            summary = {
                "cpython": cpython,
                "imre": imre,
                "speedup_ratio_of_medians": speedup,
            }
            baseline_message = ""
            if provenance is not None:
                baseline = shared.summary(phases["samples"], "baseline")
                baseline_speedup = baseline["median_seconds"] / imre["median_seconds"]
                summary.update(
                    {
                        "baseline": baseline,
                        "baseline_to_imre_ratio_of_medians": baseline_speedup,
                        "cpython_to_baseline_ratio_of_medians": cpython[
                            "median_seconds"
                        ]
                        / baseline["median_seconds"],
                    }
                )
                baseline_message = (
                    f", baseline {baseline['median_seconds']:.6f}s, "
                    f"{baseline_speedup:.2f}x versus baseline"
                )
            workloads.append(
                {
                    "shape": shape,
                    "tasks": size,
                    "dependencies": sum(len(task.dependencies) for task in tasks),
                    "seed": args.seed,
                    "lookup": lookup,
                    "input": stdin.decode("utf-8"),
                    "input_sha256": hashlib.sha256(stdin).hexdigest(),
                    "expected_stdout": expected,
                    "expected_stdout_sha256": hashlib.sha256(
                        expected.encode("utf-8")
                    ).hexdigest(),
                    "expected_stdout_bytes": len(expected.encode("utf-8")),
                    **phases,
                    "summary": summary,
                }
            )
            print(
                f"{shape}, {size} tasks: CPython {cpython['median_seconds']:.6f}s, "
                f"Imre {imre['median_seconds']:.6f}s, {speedup:.2f}x{baseline_message}",
                flush=True,
            )
    if validator.package_sources() != source_paths:
        raise RuntimeError("package source files changed during the benchmark")
    for path in source_paths:
        name = str(path.relative_to(validator.PACKAGE_ROOT))
        if shared.sha256(path) != source_hashes[name]:
            raise RuntimeError(f"{name} changed during the benchmark")
    for name, path in inputs.items():
        if shared.sha256(path) != hashes[name]:
            raise RuntimeError(f"{name} changed during the benchmark")
    result = {
        "schema_version": 2,
        "example": "dependency_planner",
        "completed_at_utc": datetime.now(timezone.utc).isoformat(),
        "git": {"revision": revision, "dirty": dirty},
        "sources": source_hashes,
        "sha256": hashes,
        "tools": {
            "imre": compiler_version,
            "clang": clang_version,
            "cpython": python_info,
            "harness_python": sys.version,
        },
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
            "seed": args.seed,
            "samples": args.samples,
            "warmups": args.warmups,
            "timeout_seconds": args.timeout,
            "llvm_optimization": "O3",
        },
        "method": {
            "timer": "time.perf_counter_ns around subprocess.run; shared with benchmark.py",
            "includes": "fresh process startup, package imports, input parsing, all scheduling and complete report output, and process exit",
            "excludes": "Rust workspace builds, AOT compilation, oracle/differential validation, and warmups",
            "ordering": (
                "all six CPython/baseline/Imre orders in a repeating cycle; adjacent orders reverse each other"
                if provenance is not None
                else "alternating CPython-first and Imre-first pairs, starting with CPython"
            ),
            "fully_balanced_samples": args.samples % len(orders) == 0,
            "cpu": "harness and children inherit the same single-CPU affinity",
            "output_validation": "every warmup and sample must match the complete oracle output, exit zero, and emit no stderr; comparison and UTF-8 output hashing occur after timing",
            "output_storage": "complete expected_stdout appears once per workload; each raw run retains duration, exit status, stderr, stdout SHA256, and stdout byte length",
            "cpython_command": [
                python_path.name,
                "-I",
                "-c",
                validator.BOOTSTRAP,
                "<package-root>",
            ],
            "cpython_imports": "explicit package-root bootstrap runs unchanged package as __main__; normal bytecode caching is allowed",
            "imre_command": ["<artifact-dir>/dependency_planner"],
            "compile_command": [
                "imre",
                "build",
                "examples/dependency_planner/__main__.py",
                "--module-root",
                "examples",
                "-o",
                "<artifact-dir>/dependency_planner",
                "--runtime",
                "<runtime.a>",
                "--clang",
                Path(clang).name,
                "--opt-level",
                "3",
                "--emit-llvm",
                "<artifact-dir>/dependency_planner.ll",
            ],
        },
        "compilation": compilation,
        "artifacts": {
            "executable_bytes": binary.stat().st_size,
            "llvm_ir_bytes": llvm.stat().st_size,
        },
        "validation": validation,
        "workloads": workloads,
    }
    if provenance is not None:
        result["baseline_build_record"] = provenance
        result["baseline_validation"] = baseline_validation
        result["artifacts"]["baseline_executable_bytes"] = (
            inputs["baseline"].stat().st_size
        )
        result["method"].update(
            {
                "order_cycle": orders,
                "baseline_command": ["<baseline-executable>"],
                "baseline_provenance": "required build record matches the baseline executable and every package source hash; baseline executable and record are checked again after measurement",
            }
        )
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True)
    parser.add_argument("--runtime", required=True, type=Path)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--baseline-record", type=Path)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--clang", default="clang")
    parser.add_argument(
        "--output", type=Path, default=shared.LOCAL / "dependency-planner.json"
    )
    parser.add_argument(
        "--artifact-dir", type=Path, default=shared.LOCAL / "dependency-planner"
    )
    parser.add_argument(
        "--sizes", type=shared.positive_int, nargs="+", default=[128, 512, 2048]
    )
    parser.add_argument(
        "--shapes",
        choices=("layered", "wide", "chain"),
        nargs="+",
        default=["layered", "wide"],
    )
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--samples", type=shared.positive_int, default=12)
    parser.add_argument("--warmups", type=shared.positive_int, default=2)
    parser.add_argument("--cpu", type=int)
    parser.add_argument("--timeout", type=shared.positive_float, default=120.0)
    args = parser.parse_args()
    if (args.baseline is None) != (args.baseline_record is None):
        parser.error("--baseline and --baseline-record must be supplied together")
    result = benchmark(args)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.output.with_name(args.output.name + ".tmp")
    temporary.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")
    temporary.replace(args.output)
    print(f"Saved {args.output}")


if __name__ == "__main__":
    main()
