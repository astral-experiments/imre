"""Compare unchanged incremental-planner Python with Imre and two Rust modes."""

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
REFERENCE = HERE / "reference/incremental_planner"
helpers = SimpleNamespace(**runpy.run_path(str(HERE / "dependency_compare.py")))
shared = helpers.shared
validator = SimpleNamespace(**runpy.run_path(str(HERE / "incremental_validate.py")))
ENGINES = ("cpython", "imre", "rust_bigint", "rust_i64")
ORDERS = tuple(
    tuple(ENGINES[(value + shift) % 4] for value in (0, 1, 3, 2)) for shift in range(4)
)
DEFAULT_WORKLOADS = [
    ("layered", 16384, "sparse"),
    ("layered", 16384, "all"),
    ("wide", 16384, "sparse"),
    ("wide", 16384, "all"),
    ("boundary", 128, "all"),
]


def workload_spec(value: str) -> tuple[str, int, str]:
    try:
        shape, count, changes = value.split(":")
        size = shared.positive_int(count)
    except (ValueError, argparse.ArgumentTypeError) as error:
        raise argparse.ArgumentTypeError(
            "expected SHAPE:POSITIVE_SIZE:CHANGES"
        ) from error
    if shape not in {"layered", "wide", "boundary"} or changes not in {
        "sparse",
        "all",
        "none",
    }:
        raise argparse.ArgumentTypeError(
            "shape: layered/wide/boundary; changes: sparse/all/none"
        )
    return shape, size, changes


def check_build_mode(record: dict, thin_lto: bool) -> None:
    environment = record.get("environment", {})
    if environment.get("CARGO_PROFILE_RELEASE_OPT_LEVEL", "3") != "3":
        raise ValueError("Rust references must use release optimization level 3")
    if thin_lto:
        expected = (
            "-Cllvm-args=-import-instr-limit=1000 -Cllvm-args=-inline-threshold=1000"
        )
        if (
            environment.get("CARGO_PROFILE_RELEASE_LTO") != "thin"
            or record.get("llvm_budget") != 1000
            or environment.get("RUSTFLAGS") != expected
            or environment.get("CARGO_ENCODED_RUSTFLAGS") is not None
        ):
            raise ValueError(
                "ThinLTO comparison requires Rust ThinLTO with explicit LLVM budgets of 1000"
            )
    elif (
        environment.get("CARGO_PROFILE_RELEASE_LTO") not in (None, "false", "off")
        or record.get("llvm_budget") is not None
        or environment.get("RUSTFLAGS") != ""
        or environment.get("CARGO_ENCODED_RUSTFLAGS") != ""
    ):
        raise ValueError(
            "ordinary comparison requires ordinary Rust release builds without custom LLVM flags"
        )


def benchmark(args: argparse.Namespace) -> dict:
    started = datetime.now(timezone.utc).isoformat()
    cpu = shared.pin_cpu(args.cpu)
    artifacts = args.artifact_dir.resolve()
    binary = artifacts / "incremental_planner"
    llvm = artifacts / "incremental_planner.ll"
    source_root = args.rust_source_root.resolve(strict=True)
    inputs = {
        "compiler": Path(shared.executable(str(args.compiler))),
        "runtime": args.runtime.resolve(strict=True),
        "clang": Path(shared.executable(args.clang)),
        "rust_bigint": Path(shared.executable(str(args.rust_bigint))),
        "rust_i64": Path(shared.executable(str(args.rust_i64))),
        "rust_record": args.rust_build_record.resolve(strict=True),
        "rust_build_script": source_root / "build.py",
        "harness": Path(__file__).resolve(),
        "validator": HERE / "incremental_validate.py",
        "comparison_helpers": HERE / "dependency_compare.py",
        "dependency_harness": HERE / "dependency_benchmark.py",
        "shared_harness": HERE / "benchmark.py",
        "route_validator": HERE / "validate.py",
        "dependency_validator": HERE / "dependency_validate.py",
        "guard_helpers": HERE / "reference/dependency_planner/check_boundaries.py",
    }
    flags = []
    recorded_flags = []
    if args.lto_linker is not None:
        # Keep argv[0]'s lexical ld.lld name; resolve only its file identity.
        inputs["lto_linker"] = args.lto_linker.absolute()
        flags += ["--lto-linker", str(inputs["lto_linker"])]
        recorded_flags += ["--lto-linker", f"<linker-dir>/{inputs['lto_linker'].name}"]
    if args.target is not None:
        flags += ["--target", args.target]
        recorded_flags += ["--target", args.target]
    if args.imre_build_record is not None:
        inputs["imre_build_record"] = args.imre_build_record.resolve(strict=True)
    python = shared.executable(args.python)
    python_info = json.loads(
        shared.run([python, "-I", "-c", shared.PYTHON_INFO], b"", args.timeout)[
            "stdout"
        ]
    )
    if python_info["implementation"] != "cpython":
        raise ValueError("--python must select CPython")
    inputs["python"] = Path(python_info.pop("executable")).resolve()
    python_info["executable_name"] = inputs["python"].name
    source_paths = validator.package_sources()
    sources = {
        str(path.relative_to(validator.PACKAGE_ROOT)): shared.sha256(path)
        for path in source_paths
    }
    rust_sources = helpers.rust_sources(source_root)
    if helpers.rust_sources(REFERENCE) != rust_sources:
        raise ValueError("saved Rust sources differ from the tracked reference")
    hashes = {name: shared.sha256(path) for name, path in inputs.items()}
    rust_record = helpers.verify_rust_record(
        inputs["rust_record"], rust_sources, hashes
    )
    check_build_mode(rust_record, args.lto_linker is not None)
    protected = [
        *inputs.values(),
        *source_paths,
        *(source_root / name for name in rust_sources),
        *(REFERENCE / name for name in rust_sources),
    ]
    imre_record = None
    if args.imre_build_record is not None:
        original = json.loads(inputs["imre_build_record"].read_text())
        if not {hashes["compiler"], hashes["runtime"]} <= set(
            original.get("sha256", {}).values()
        ):
            raise ValueError(
                "Imre build record does not identify the selected compiler/runtime"
            )
        for name, digest in original.get("source_sha256", {}).items():
            path = ROOT / name
            if shared.sha256(path) != digest:
                raise ValueError(f"Imre build source changed: {name}")
            inputs[f"imre_source:{name}"] = path
            hashes[f"imre_source:{name}"] = digest
            protected.append(path)
        imre_record = {
            key: original[key]
            for key in (
                "git_head",
                "git_status",
                "rustc_verbose",
                "source_sha256",
                "sha256",
                "runtime_lto_rustflags",
            )
            if key in original
        }
    helpers.boundaries.protect_outputs(
        [binary, llvm, args.output, args.output.with_name(args.output.name + ".tmp")],
        protected,
    )
    if inputs["rust_bigint"].samefile(inputs["rust_i64"]):
        raise ValueError("the two Rust numeric modes must use distinct executables")
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
    target_flags = [f"--target={args.target}"] if args.target else []
    effective_target = shared.run(
        [str(inputs["clang"]), *target_flags, "-dumpmachine"], b"", args.timeout
    )["stdout"].strip()
    linker_version = (
        shared.run([str(inputs["lto_linker"]), "--version"], b"", args.timeout)[
            "stdout"
        ].strip()
        if args.lto_linker is not None
        else None
    )
    artifacts.mkdir(parents=True, exist_ok=True)
    command = [
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
        *flags,
    ]
    compilation = shared.run(command, b"", args.timeout)
    inputs.update(binary=binary, llvm_ir=llvm)
    hashes.update({name: shared.sha256(inputs[name]) for name in ("binary", "llvm_ir")})
    commands = {
        "cpython": validator.python_command(str(inputs["python"])),
        "imre": [str(binary)],
        "rust_bigint": [str(inputs["rust_bigint"])],
        "rust_i64": [str(inputs["rust_i64"])],
    }
    validation = validator.validate_commands(commands, args.timeout)
    workloads = []
    for shape, size, changes in args.workloads:
        case = validator.make_workload(size, args.seed, shape, changes)
        expected = validator.oracle(case.tasks, case.changes)
        exclusion = validator.i64_exclusion(case.tasks, expected)
        if expected["kind"] != "OK" or exclusion is not None:
            raise ValueError(
                f"measured workload is outside the common domain: {case.label}: {exclusion}"
            )
        stdin = validator.input_text(case.tasks, case.changes).encode()
        output = expected["output"]
        # Every workload passes all engines before either warmups or timing.
        for name, invocation in commands.items():
            if shared.run(invocation, stdin, args.timeout)["stdout"] != output:
                raise AssertionError(
                    f"{name}/{case.label}: workload validation mismatch"
                )
        phases = {"warmups": [], "samples": []}
        for phase, count in (("warmups", args.warmups), ("samples", args.samples)):
            for index in range(count):
                order = ORDERS[index % len(ORDERS)]
                sample = {"index": index, "order": order}
                for name in order:
                    result = shared.run(commands[name], stdin, args.timeout)
                    if result["stdout"] != output:
                        raise AssertionError(
                            f"{name}/{case.label}: {phase} {index} complete-output mismatch"
                        )
                    sample[name] = helpers.dependency.compact_output(result)
                phases[phase].append(sample)
        summaries = {name: shared.summary(phases["samples"], name) for name in ENGINES}
        medians = {
            name: summary["median_seconds"] for name, summary in summaries.items()
        }
        ratios = {
            f"{a}_over_{b}": medians[a] / medians[b]
            for a, b in (
                ("cpython", "imre"),
                ("imre", "rust_bigint"),
                ("imre", "rust_i64"),
                ("cpython", "rust_bigint"),
                ("cpython", "rust_i64"),
                ("rust_bigint", "rust_i64"),
            )
        }
        workloads.append(
            {
                "label": case.label,
                "shape": shape,
                "tasks": size,
                "changes": changes,
                "seed": args.seed,
                "changed": len(set(case.changes)),
                "affected": len(expected["order"]),
                "dependencies": sum(len(task.dependencies) for task in case.tasks),
                "input": stdin.decode(),
                "input_sha256": hashlib.sha256(stdin).hexdigest(),
                "expected_stdout": output,
                "expected_stdout_sha256": hashlib.sha256(output.encode()).hexdigest(),
                "expected_stdout_bytes": len(output.encode()),
                **phases,
                "summary": {**summaries, "ratios_of_medians": ratios},
            }
        )
        print(
            case.label
            + ": "
            + ", ".join(f"{name} {medians[name] * 1000:.3f}ms" for name in ENGINES),
            flush=True,
        )
    if (
        validator.package_sources() != source_paths
        or helpers.rust_sources(source_root) != rust_sources
        or helpers.rust_sources(REFERENCE) != rust_sources
    ):
        raise RuntimeError("Python or Rust source set changed during comparison")
    for path in source_paths:
        if (
            shared.sha256(path)
            != sources[str(path.relative_to(validator.PACKAGE_ROOT))]
        ):
            raise RuntimeError("Python source changed during comparison")
    for name, path in inputs.items():
        if shared.sha256(path) != hashes[name]:
            raise RuntimeError(f"{name} changed during comparison")
    return {
        "schema_version": 1,
        "example": "incremental_planner",
        "started_at_utc": started,
        "completed_at_utc": datetime.now(timezone.utc).isoformat(),
        "git": {"revision": revision, "dirty": dirty},
        "sources": sources,
        "rust_sources": rust_sources,
        "sha256": hashes,
        "artifacts": {
            name: {"bytes": inputs[name].stat().st_size}
            for name in ("binary", "llvm_ir", "rust_bigint", "rust_i64")
        },
        "rust_build_record": rust_record,
        "imre_build_provenance": imre_record,
        "tools": {
            "imre": compiler_version,
            "clang": clang_version,
            "lto_linker": linker_version,
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
            "optimization_level": 3,
            "target": effective_target,
            "requested_target": args.target,
            "lto": "thin" if args.lto_linker is not None else None,
            "llvm_budget": 1000 if args.lto_linker is not None else None,
            "workloads": args.workloads,
            "seed": args.seed,
            "samples": args.samples,
            "warmups": args.warmups,
            "timeout_seconds": args.timeout,
        },
        "method": {
            "timer": "time.perf_counter_ns around subprocess.run via benchmark.py",
            "includes": "fresh process startup, pipes, input parsing, whole-graph validation, affected closure, rebuild scheduling, complete report and exit; CPython imports the unchanged package",
            "excludes": "builds, independent oracle, correctness gates, workload generation, warmups, output comparison and hashes",
            "order_cycle": ORDERS,
            "fully_balanced_samples": True,
            "ordering": "four Williams orders repeat; each cycle places every engine in every position once and every directed within-round adjacent pair once; cross-round predecessors are not balanced",
            "cpu": "harness and children inherit one CPU affinity; traced timing is refused",
            "hash_seeds": "fresh processes use each implementation's default hash seeding; output is independent of hash iteration order",
            "output": "complete expected stdout retained once per workload; each run matches it exactly before retaining duration/status/stderr, stdout SHA256 and length",
            "compile_command": [
                inputs["compiler"].name,
                "build",
                "examples/incremental_planner/__main__.py",
                "--module-root",
                "examples",
                "-o",
                "<artifact-dir>/incremental_planner",
                "--runtime",
                "<runtime>",
                "--clang",
                inputs["clang"].name,
                "--opt-level",
                "3",
                "--emit-llvm",
                "<artifact-dir>/incremental_planner.ll",
                *recorded_flags,
            ],
            "commands": {
                "cpython": [
                    inputs["python"].name,
                    "-I",
                    "-c",
                    validator.BOOTSTRAP,
                    "<package-root>",
                ],
                "imre": ["<imre-binary>"],
                "rust_bigint": ["<rust-bigint-binary>"],
                "rust_i64": ["<rust-i64-binary>"],
            },
            "scope": "workload-specific whole-process comparison; Rust directly ports the algorithm but uses different source, native indices and representations; checked i64 has a narrower numerical contract",
        },
        "validation": validation,
        "workloads": workloads,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--imre-build-record", type=Path)
    parser.add_argument("--rust-bigint", type=Path, required=True)
    parser.add_argument("--rust-i64", type=Path, required=True)
    parser.add_argument("--rust-build-record", type=Path, required=True)
    parser.add_argument("--rust-source-root", type=Path, default=REFERENCE)
    parser.add_argument("--python", default="python3.14")
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--target")
    parser.add_argument(
        "--artifact-dir", type=Path, default=HERE / "local/incremental-planner"
    )
    parser.add_argument(
        "--workload",
        type=workload_spec,
        action="append",
        dest="workloads",
        help="repeat SHAPE:SIZE:CHANGES; defaults to four 16K graphs and one 128-target numeric boundary graph",
    )
    parser.add_argument("--seed", type=int, default=20261017)
    parser.add_argument("--samples", type=shared.positive_int, default=32)
    parser.add_argument("--warmups", type=shared.positive_int, default=2)
    parser.add_argument("--cpu", type=int, help="default: first allowed CPU")
    parser.add_argument("--timeout", type=shared.positive_float, default=120.0)
    parser.add_argument(
        "--output", type=Path, default=HERE / "local/incremental-comparison.json"
    )
    args = parser.parse_args()
    args.workloads = args.workloads or DEFAULT_WORKLOADS
    if args.samples % len(ORDERS):
        parser.error(
            "--samples must be a multiple of four for balanced execution order"
        )
    if args.lto_linker is not None and args.target is None:
        parser.error("--lto-linker requires an explicit --target")
    if len(set(args.workloads)) != len(args.workloads):
        parser.error("workloads must be distinct")
    result = benchmark(args)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.output.with_name(args.output.name + ".tmp")
    temporary.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")
    temporary.replace(args.output)
    print(f"Saved {args.output}")


if __name__ == "__main__":
    main()
