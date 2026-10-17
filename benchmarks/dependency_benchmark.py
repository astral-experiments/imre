"""Benchmark the unchanged dependency-planner package on CPython and Relar."""

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


def compact_output(result: dict) -> dict:
    """Retain output evidence after the caller checks the complete output."""
    stdout = result["stdout"].encode("utf-8")
    return {
        **{key: value for key, value in result.items() if key != "stdout"},
        "stdout_sha256": hashlib.sha256(stdout).hexdigest(),
        "stdout_bytes": len(stdout),
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
    commands = {"cpython": validator.python_command(python), "relar": [str(binary)]}
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
                    order = (
                        ["cpython", "relar"] if index % 2 == 0 else ["relar", "cpython"]
                    )
                    pair = {"index": index, "order": order}
                    for implementation in order:
                        result = shared.run(
                            commands[implementation], stdin, args.timeout
                        )
                        if result["stdout"] != expected:
                            raise AssertionError(
                                f"output mismatch for {implementation}, {shape}, "
                                f"{size} tasks, {phase} {index}"
                            )
                        pair[implementation] = compact_output(result)
                    phases[phase].append(pair)
            cpython = shared.summary(phases["samples"], "cpython")
            relar = shared.summary(phases["samples"], "relar")
            speedup = cpython["median_seconds"] / relar["median_seconds"]
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
                    "summary": {
                        "cpython": cpython,
                        "relar": relar,
                        "speedup_ratio_of_medians": speedup,
                    },
                }
            )
            print(
                f"{shape}, {size} tasks: CPython {cpython['median_seconds']:.6f}s, "
                f"Relar {relar['median_seconds']:.6f}s, {speedup:.2f}x",
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
    return {
        "schema_version": 2,
        "example": "dependency_planner",
        "completed_at_utc": datetime.now(timezone.utc).isoformat(),
        "git": {"revision": revision, "dirty": dirty},
        "sources": source_hashes,
        "sha256": hashes,
        "tools": {
            "relar": compiler_version,
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
            "ordering": "alternating CPython-first and Relar-first pairs, starting with CPython",
            "fully_balanced_samples": args.samples % 2 == 0,
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
            "relar_command": ["<artifact-dir>/dependency_planner"],
            "compile_command": [
                "relar",
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


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True)
    parser.add_argument("--runtime", required=True, type=Path)
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
    result = benchmark(args)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.output.with_name(args.output.name + ".tmp")
    temporary.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")
    temporary.replace(args.output)
    print(f"Saved {args.output}")


if __name__ == "__main__":
    main()
