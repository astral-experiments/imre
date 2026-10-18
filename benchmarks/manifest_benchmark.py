"""Compare unchanged-program controls and file-backed manifest workloads."""

import argparse
import hashlib
import itertools
import json
import platform
import runpy
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
REFERENCE = HERE / "reference/incremental_planner"
batch = SimpleNamespace(**runpy.run_path(str(HERE / "batch_benchmark.py")))
existing = batch.existing
helpers = existing.helpers
shared = existing.shared
validator = SimpleNamespace(**runpy.run_path(str(HERE / "manifest_validate.py")))
incremental = validator.batch.incremental
CONTROL_ENGINES = ("cpython", "baseline", "relar")
MANIFEST_ENGINES = ("cpython", "relar", "rust_bigint")


def text_hash(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()


def rust_record(path: Path, sources: dict, hashes: dict) -> dict:
    record = json.loads(path.read_text())
    if record.get("entrypoint") != "manifest" or record.get("sources") != sources:
        raise ValueError(
            "Rust record must identify the exact manifest reference sources"
        )
    contents = record.get("source_contents", {})
    if set(contents) != set(sources) or any(
        text_hash(contents[name]) != digest for name, digest in sources.items()
    ):
        raise ValueError("Rust record must preserve every exact source file")
    if (
        record.get("binaries", {}).get("bigint", {}).get("sha256")
        != hashes["rust_bigint"]
    ):
        raise ValueError(
            "Rust record does not identify the selected manifest executable"
        )
    if (
        text_hash(record.get("build_script", "")) != hashes["rust_build_script"]
        or record.get("build_script_sha256") != hashes["rust_build_script"]
    ):
        raise ValueError("Rust build helper differs from its recorded source")
    if not record.get("rustc_verbose") or not record.get("cargo_version"):
        raise ValueError("Rust record must include exact compiler and Cargo versions")
    return record


select_workloads = batch.select_workloads
captured_workload = batch.captured_workload
measure_control = batch.measure


def manifest_workload(workload: dict, mixed: bool, repetitions: int) -> dict:
    paths = []
    files = {"payload files/valid graph.txt": workload["input"].encode()}
    if mixed:
        files.update(
            {
                "payload files/bad integer.txt": b"1\nbroken x\n0\n",
                "payload files/bad utf8.txt": b"\xff",
            }
        )
    for _ in range(repetitions):
        paths.append("payload files/valid graph.txt")
        if mixed:
            paths.extend(
                (
                    "payload files/bad integer.txt",
                    "missing payload.txt",
                    "payload files/bad utf8.txt",
                )
            )
    files["manifest.txt"] = ("\n".join(paths) + "\n").encode()
    case = validator.Case(
        workload["label"] + ("-mixed" if mixed else "-valid"),
        files,
        ("manifest.txt", "reports/plan.txt"),
        ("reports",),
    )
    expected = validator.oracle(case)
    if expected["returncode"] != 0:
        raise AssertionError("benchmark manifest must finish successfully")
    return {
        "label": case.label,
        "kind": "mixed_recovered_input_errors" if mixed else "all_valid",
        "source_workload": workload["label"],
        "valid_payload_repetitions": repetitions,
        "case": validator.case_record(case),
        "requests": expected["requests"],
        "completed_requests": expected["completed"],
        "invalid_requests": expected["invalid"],
        "cleaned_requests": expected["cleaned"],
        "expected_stdout": expected["stdout"],
        "expected_changed_files": validator.encoded_files(
            {
                name: data
                for name, data in expected["files"].items()
                if case.files.get(name) != data
            }
        ),
        "expected_file_inventory": {
            name: {"sha256": validator.digest(data), "bytes": len(data)}
            for name, data in sorted(expected["files"].items())
        },
    }


def measure_manifest(workload: dict, commands: dict, args: argparse.Namespace) -> dict:
    case = validator.case_from_record(workload["case"])
    expected = validator.oracle(case)
    sandbox_root = args.artifact_dir.resolve() / "sandboxes"
    sandbox_root.mkdir(parents=True, exist_ok=True)

    def run(name: str) -> dict:
        with tempfile.TemporaryDirectory(
            prefix=name + "-", dir=sandbox_root
        ) as temporary:
            directory = Path(temporary)
            validator.materialize(case, directory)
            started = time.perf_counter_ns()
            process = subprocess.run(
                [*commands[name], *case.arguments],
                input=b"",
                capture_output=True,
                cwd=directory,
                timeout=args.timeout,
                check=False,
            )
            elapsed = time.perf_counter_ns() - started
            return {
                "elapsed_ns": elapsed,
                **validator.check_result(case, expected, process, directory),
            }

    for name in commands:
        run(name)
    orders = tuple(itertools.permutations(commands))
    phases = {"warmups": [], "samples": []}
    for phase, count in (("warmups", args.warmups), ("samples", args.samples)):
        for index in range(count):
            order = orders[index % len(orders)]
            sample = {"index": index, "order": order}
            for name in order:
                sample[name] = run(name)
            phases[phase].append(sample)
    summaries = {name: shared.summary(phases["samples"], name) for name in commands}
    medians = {name: value["median_seconds"] for name, value in summaries.items()}
    summaries["ratios_of_medians"] = {
        f"{left}_over_{right}": medians[left] / medians[right]
        for left, right in itertools.permutations(commands, 2)
    }
    print(
        workload["label"]
        + ": "
        + ", ".join(f"{name} {value * 1000:.3f}ms" for name, value in medians.items()),
        flush=True,
    )
    return {**workload, **phases, "summary": summaries}


def benchmark(args: argparse.Namespace) -> dict:
    started = datetime.now(timezone.utc).isoformat()
    cpu = shared.pin_cpu(args.cpu)
    artifacts = args.artifact_dir.resolve()
    source_root = args.rust_source_root.resolve(strict=True)
    inputs = {
        "compiler": Path(shared.executable(str(args.compiler))),
        "runtime": args.runtime.resolve(strict=True),
        "clang": Path(shared.executable(args.clang)),
        "baseline": Path(shared.executable(str(args.baseline))),
        "baseline_record": args.baseline_record.resolve(strict=True),
        "rust_bigint": Path(shared.executable(str(args.rust_bigint))),
        "rust_record": args.rust_build_record.resolve(strict=True),
        "rust_build_script": source_root / "build.py",
        "harness": Path(__file__).resolve(),
        **{
            name: HERE / path
            for name, path in {
                "validator": "manifest_validate.py",
                "batch_validator": "batch_validate.py",
                "batch_harness": "batch_benchmark.py",
                "incremental_validator": "incremental_validate.py",
                "incremental_harness": "incremental_benchmark.py",
                "comparison_helpers": "dependency_compare.py",
                "dependency_harness": "dependency_benchmark.py",
                "shared_harness": "benchmark.py",
                "route_validator": "validate.py",
                "dependency_validator": "dependency_validate.py",
                "guard_helpers": "reference/dependency_planner/check_boundaries.py",
            }.items()
        },
    }
    flags = []
    recorded_flags = []
    if args.lto_linker is not None:
        inputs["lto_linker"] = args.lto_linker.absolute()
        flags += ["--lto-linker", str(inputs["lto_linker"])]
        recorded_flags += ["--lto-linker", f"<linker-dir>/{inputs['lto_linker'].name}"]
    if args.target is not None:
        flags += ["--target", args.target]
        recorded_flags += ["--target", args.target]
    if args.relar_build_record is not None:
        inputs["relar_build_record"] = args.relar_build_record.resolve(strict=True)
    python_info = json.loads(
        shared.run(
            [shared.executable(args.python), "-I", "-c", shared.PYTHON_INFO],
            b"",
            args.timeout,
        )["stdout"]
    )
    if python_info["implementation"] != "cpython":
        raise ValueError("--python must select CPython")
    inputs["python"] = Path(python_info.pop("executable")).resolve(strict=True)
    python_info["executable_name"] = inputs["python"].name
    source_paths = validator.package_sources()
    sources = {
        str(path.relative_to(validator.PACKAGE_ROOT)): shared.sha256(path)
        for path in source_paths
    }
    old_sources = {
        str(path.relative_to(validator.PACKAGE_ROOT)): shared.sha256(path)
        for path in incremental.package_sources()
    }
    rust_sources = helpers.rust_sources(source_root)
    if helpers.rust_sources(REFERENCE) != rust_sources:
        raise ValueError(
            "saved Rust sources differ from the tracked manifest reference"
        )
    hashes = {name: shared.sha256(path) for name, path in inputs.items()}
    reference_record = rust_record(inputs["rust_record"], rust_sources, hashes)
    thin_lto = args.lto_linker is not None
    existing.check_build_mode(reference_record, thin_lto)
    baseline = json.loads(inputs["baseline_record"].read_text())
    if (
        baseline.get("example") != "exception_performance"
        or {
            name: value
            for name, value in baseline.get("sources", {}).items()
            if name.startswith("incremental_planner/")
        }
        != old_sources
        or baseline.get("sha256", {}).get("control_binary") != hashes["baseline"]
        or baseline.get("options", {}).get("lto") != ("thin" if thin_lto else None)
        or baseline.get("options", {}).get("llvm_budget")
        != (1000 if thin_lto else None)
    ):
        raise ValueError(
            "baseline must identify the unchanged incremental source/binary and matching LTO mode"
        )
    protected = [
        *inputs.values(),
        *source_paths,
        *(source_root / name for name in rust_sources),
        *(REFERENCE / name for name in rust_sources),
        REFERENCE / "build.py",
    ]
    relar_record = None
    if args.relar_build_record is not None:
        original = json.loads(inputs["relar_build_record"].read_text())
        if not {hashes["compiler"], hashes["runtime"]} <= set(
            original.get("sha256", {}).values()
        ):
            raise ValueError(
                "Relar build record does not identify the selected compiler/runtime"
            )
        if thin_lto and original.get("runtime_lto_rustflags") != "-Clinker-plugin-lto":
            raise ValueError(
                "Relar build record must identify the linker-plugin-LTO runtime build"
            )
        runtime_name = "librelar_runtime_lto.a" if thin_lto else "librelar_runtime.a"
        if not any(
            Path(name).name == runtime_name and digest == hashes["runtime"]
            for name, digest in original["sha256"].items()
        ):
            raise ValueError("Relar build record identifies a different runtime mode")
        for name, digest in original.get("source_sha256", {}).items():
            path = ROOT / name
            if shared.sha256(path) != digest:
                raise ValueError(f"Relar build source changed: {name}")
            inputs[f"relar_source:{name}"] = path
            hashes[f"relar_source:{name}"] = digest
            protected.append(path)
        relar_record = {
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
    outputs = {
        name: artifacts / filename
        for name, filename in {
            "control_binary": "incremental_planner",
            "control_ir": "incremental_planner.ll",
            "manifest_binary": "manifest_planner",
            "manifest_ir": "manifest_planner.ll",
        }.items()
    }
    helpers.boundaries.protect_outputs(
        [
            *outputs.values(),
            args.output,
            args.output.with_name(args.output.name + ".tmp"),
        ],
        protected,
    )
    revision = shared.run(
        ["git", "-C", str(ROOT), "rev-parse", "HEAD"], b"", args.timeout
    )["stdout"].strip()
    dirty = bool(
        shared.run(
            ["git", "-C", str(ROOT), "status", "--porcelain"], b"", args.timeout
        )["stdout"]
    )
    target_flags = [f"--target={args.target}"] if args.target else []
    effective_target = shared.run(
        [str(inputs["clang"]), *target_flags, "-dumpmachine"], b"", args.timeout
    )["stdout"].strip()
    if baseline["options"]["target"] != effective_target:
        raise ValueError("baseline and current Clang effective targets differ")
    tools = {
        name: shared.run([str(inputs[key]), "--version"], b"", args.timeout)[
            "stdout"
        ].strip()
        for name, key in (("relar", "compiler"), ("clang", "clang"))
    }
    tools["lto_linker"] = (
        shared.run([str(inputs["lto_linker"]), "--version"], b"", args.timeout)[
            "stdout"
        ].strip()
        if thin_lto
        else None
    )
    tools.update(cpython=python_info, harness_python=sys.version)
    artifacts.mkdir(parents=True, exist_ok=True)
    compilations = {}
    compile_commands = {}
    for name, entry in (("control", incremental.ENTRY), ("manifest", validator.ENTRY)):
        command = [
            str(inputs["compiler"]),
            "build",
            str(entry),
            "--module-root",
            str(validator.PACKAGE_ROOT),
            "-o",
            str(outputs[name + "_binary"]),
            "--runtime",
            str(inputs["runtime"]),
            "--clang",
            str(inputs["clang"]),
            "--opt-level",
            "3",
            "--emit-llvm",
            str(outputs[name + "_ir"]),
            *flags,
        ]
        compilations[name] = shared.run(command, b"", args.timeout)
        compile_commands[name] = [
            inputs["compiler"].name,
            "build",
            str(entry.relative_to(ROOT)),
            "--module-root",
            "examples",
            "-o",
            "<artifact-dir>/" + outputs[name + "_binary"].name,
            "--runtime",
            "<runtime>",
            "--clang",
            inputs["clang"].name,
            "--opt-level",
            "3",
            "--emit-llvm",
            "<artifact-dir>/" + outputs[name + "_ir"].name,
            *recorded_flags,
        ]
    inputs.update(outputs)
    hashes.update({name: shared.sha256(path) for name, path in outputs.items()})
    commands = {
        "control": {
            "cpython": incremental.python_command(str(inputs["python"])),
            "baseline": [str(inputs["baseline"])],
            "relar": [str(outputs["control_binary"])],
        },
        "manifest": {
            "cpython": validator.python_command(str(inputs["python"])),
            "relar": [str(outputs["manifest_binary"])],
            "rust_bigint": [str(inputs["rust_bigint"])],
        },
    }
    validation = {
        "control": incremental.validate_commands(commands["control"], args.timeout),
        "manifest": validator.validate_commands(commands["manifest"], args.timeout),
    }
    captured = {"workloads": baseline["workloads"]["control"]}
    controls = [
        captured_workload(workload)
        for workload in select_workloads(captured, args.control_workloads)
    ]
    seeds = select_workloads(captured, args.manifest_workloads)
    if args.manifest_workloads is None:
        seeds = [workload for workload in seeds if workload["label"].endswith("-all")]
    manifests = [
        manifest_workload(captured_workload(workload), mixed, args.requests)
        for workload in seeds
        for mixed in (False, True)
    ]
    workloads = {
        "control": [
            measure_control(workload, commands["control"], args)
            for workload in controls
        ],
        "manifest": [
            measure_manifest(workload, commands["manifest"], args)
            for workload in manifests
        ],
    }
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
        "example": "manifest_planner",
        "started_at_utc": started,
        "completed_at_utc": datetime.now(timezone.utc).isoformat(),
        "git": {"revision": revision, "dirty": dirty},
        "sources": sources,
        "rust_sources": rust_sources,
        "sha256": hashes,
        "artifacts": {
            name: {"bytes": path.stat().st_size}
            for name, path in inputs.items()
            if name in {*outputs, "baseline", "rust_bigint"}
        },
        "rust_build_record": reference_record,
        "relar_build_provenance": relar_record,
        "baseline_provenance": {
            "record_sha256": hashes["baseline_record"],
            **{
                key: baseline[key]
                for key in (
                    "git",
                    "sources",
                    "sha256",
                    "tools",
                    "options",
                    "compilation",
                    "relar_build_provenance",
                )
            },
            "compile_commands": baseline["method"]["compile_commands"],
        },
        "tools": tools,
        "compilation": compilations,
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
            "lto": "thin" if thin_lto else None,
            "llvm_budget": 1000 if thin_lto else None,
            "samples": args.samples,
            "warmups": args.warmups,
            "timeout_seconds": args.timeout,
            "valid_payload_repetitions": args.requests,
        },
        "method": {
            "timer": "time.perf_counter_ns around subprocess.run; shared helper for controls, explicit cwd for file workloads",
            "includes": "fresh process startup, imports, pipes, parsing, graph validation, closure, scheduling, complete output and exit; manifests also include file open/read/write/flush/close, recoverable input errors and cleanup",
            "excludes": "all builds, oracle/correctness gates, fixture directory creation and input writes, warmups, filesystem/output comparisons, hashes and fixture deletion",
            "order_cycles": {
                group: tuple(itertools.permutations(command))
                for group, command in commands.items()
            },
            "ordering": "all six permutations repeat; positions and directed within-round predecessors are balanced; cross-round predecessors are not balanced",
            "cpu": "harness and children inherit one CPU affinity; traced timing is refused",
            "output": "full control input/output and manifest input file bytes once per workload; changed output file bytes once, complete inventory hashes; every run checks entire directory snapshot plus stdout/stderr/status",
            "hash_seeds": "fresh processes retain default hash seeding; deterministic output never depends on set traversal order",
            "filesystem": "each engine execution gets a fresh isolated cwd; input fixtures are created immediately before timing and are page-cache warm; output is flushed/closed but not fsynced; this is not a cold-disk or durability benchmark",
            "compile_commands": compile_commands,
            "commands": {
                "control": {
                    "cpython": [
                        inputs["python"].name,
                        "-I",
                        "-c",
                        incremental.BOOTSTRAP,
                        "<package-root>",
                    ],
                    "baseline": ["<baseline-binary>"],
                    "relar": ["<control-binary>"],
                },
                "manifest": {
                    "cpython": [
                        inputs["python"].name,
                        "-I",
                        "-c",
                        validator.BOOTSTRAP,
                        "<package-root>",
                        "manifest.txt",
                        "reports/plan.txt",
                    ],
                    "relar": ["<manifest-binary>", "manifest.txt", "reports/plan.txt"],
                    "rust_bigint": [
                        "<rust-manifest-bigint>",
                        "manifest.txt",
                        "reports/plan.txt",
                    ],
                },
            },
            "scope": "control compares identical source/input under old/new Relar; manifest comparisons measure a new whole program, not isolated file API overhead. Rust shares the original direct-port graph algorithm/parser with native containers and Result recovery. No C or general language-speed inference.",
        },
        "validation": validation,
        "workloads": workloads,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    for option in (
        "compiler",
        "runtime",
        "baseline",
        "baseline-record",
        "rust-bigint",
        "rust-build-record",
    ):
        parser.add_argument("--" + option, type=Path, required=True)
    parser.add_argument("--relar-build-record", type=Path, required=True)
    parser.add_argument("--rust-source-root", type=Path, default=REFERENCE)
    parser.add_argument("--python", default="python3.14")
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--target")
    parser.add_argument(
        "--artifact-dir", type=Path, default=HERE / "local/manifest-planner"
    )
    parser.add_argument(
        "--control-workload",
        action="append",
        dest="control_workloads",
        metavar="CAPTURED_LABEL",
    )
    parser.add_argument(
        "--manifest-workload",
        action="append",
        dest="manifest_workloads",
        metavar="CAPTURED_LABEL",
    )
    parser.add_argument(
        "--requests",
        type=shared.positive_int,
        default=2,
        help="valid payload repetitions in each manifest; mixed adds parse/missing/UTF8 failures",
    )
    parser.add_argument("--samples", type=shared.positive_int, default=30)
    parser.add_argument("--warmups", type=shared.positive_int, default=2)
    parser.add_argument("--cpu", type=int)
    parser.add_argument("--timeout", type=shared.positive_float, default=120.0)
    parser.add_argument(
        "--output", type=Path, default=HERE / "local/manifest-comparison.json"
    )
    args = parser.parse_args()
    if args.samples % 6:
        parser.error("--samples must be a multiple of six for balanced execution order")
    if args.lto_linker is not None and args.target is None:
        parser.error("--lto-linker requires an explicit --target")
    result = benchmark(args)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.output.with_name(args.output.name + ".tmp")
    temporary.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")
    temporary.replace(args.output)
    print(f"Saved {args.output}")


if __name__ == "__main__":
    main()
