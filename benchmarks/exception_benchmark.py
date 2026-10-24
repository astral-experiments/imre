"""Compare exception optimizations against both frozen compiler/runtime baselines.

Four-engine Williams orders balance positions and within-round predecessors.
The control and batch programs, captured inputs, and full outputs stay unchanged.
"""

import argparse
import itertools
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
batch = SimpleNamespace(**runpy.run_path(str(HERE / "batch_benchmark.py")))
REFERENCE = batch.REFERENCE
existing, helpers, shared = batch.existing, batch.helpers, batch.shared
validator, incremental = batch.validator, batch.incremental


def order_cycle(commands: dict) -> tuple:
    names = dict(zip(existing.ENGINES, commands, strict=True))
    return tuple(tuple(names[name] for name in order) for order in existing.ORDERS)


def captured_batches(record: dict, labels: list[str] | None) -> list[dict]:
    selected = batch.select_workloads(
        {"workloads": record["workloads"]["batch"]}, labels
    )
    result = []
    for case in selected:
        expected = validator.oracle(case["input"])
        if (
            expected["returncode"] != 0
            or batch.text_hash(case["input"]) != case["input_sha256"]
            or expected["stdout"] != case["expected_stdout"]
            or batch.text_hash(expected["stdout"]) != case["expected_stdout_sha256"]
            or len(expected["stdout"].encode()) != case["expected_stdout_bytes"]
        ):
            raise ValueError("captured batch differs from the independent oracle")
        result.append(
            {
                key: value
                for key, value in case.items()
                if key not in {"warmups", "samples", "summary"}
            }
        )
    return result


def measure(workload: dict, commands: dict, args: argparse.Namespace) -> dict:
    stdin, output = workload["input"].encode(), workload["expected_stdout"]
    orders = order_cycle(commands)
    for name, command in commands.items():
        if shared.run(command, stdin, args.timeout)["stdout"] != output:
            raise AssertionError(f"{name}/{workload['label']}: correctness gate failed")
    phases = {"warmups": [], "samples": []}
    for phase, count in (("warmups", args.warmups), ("samples", args.samples)):
        for index in range(count):
            order = orders[index % len(orders)]
            sample = {"index": index, "order": order}
            for name in order:
                actual = shared.run(commands[name], stdin, args.timeout)
                if actual["stdout"] != output:
                    raise AssertionError(
                        f"{name}/{workload['label']}: output mismatch at {phase} {index}"
                    )
                sample[name] = helpers.dependency.compact_output(actual)
            phases[phase].append(sample)
    summaries = {name: shared.summary(phases["samples"], name) for name in commands}
    medians = {name: item["median_seconds"] for name, item in summaries.items()}
    ratios = {
        f"{left}_over_{right}": medians[left] / medians[right]
        for left, right in itertools.permutations(commands, 2)
    }
    print(
        workload["label"]
        + ": "
        + ", ".join(f"{name} {value * 1000:.3f}ms" for name, value in medians.items()),
        flush=True,
    )
    return {
        **workload,
        "input_sha256": batch.text_hash(workload["input"]),
        "expected_stdout_sha256": batch.text_hash(output),
        "expected_stdout_bytes": len(output.encode()),
        **phases,
        "summary": {**summaries, "ratios_of_medians": ratios},
    }


def saved_provenance(record: dict, digest: str) -> dict:
    keys = (
        "git",
        "sources",
        "sha256",
        "tools",
        "options",
        "compilation",
        "imre_build_provenance",
    )
    return {
        "record_sha256": digest,
        **{key: record[key] for key in keys},
        "compile_commands": record["method"].get(
            "compile_commands", record["method"].get("compile_command")
        ),
    }


def benchmark(args: argparse.Namespace) -> dict:
    started = datetime.now(timezone.utc).isoformat()
    cpu = shared.pin_cpu(args.cpu)
    artifacts = args.artifact_dir.resolve()
    source_root = args.rust_source_root.resolve(strict=True)
    inputs = {
        "compiler": Path(shared.executable(str(args.compiler))),
        "runtime": args.runtime.resolve(strict=True),
        "clang": Path(shared.executable(args.clang)),
        "pre_exception": Path(shared.executable(str(args.pre_exception_control))),
        "pre_exception_record": args.pre_exception_record.resolve(strict=True),
        "baseline_control": Path(shared.executable(str(args.baseline_control))),
        "baseline_batch": Path(shared.executable(str(args.baseline_batch))),
        "baseline_record": args.baseline_record.resolve(strict=True),
        "rust_bigint": Path(shared.executable(str(args.rust_bigint))),
        "rust_record": args.rust_build_record.resolve(strict=True),
        "rust_build_script": source_root / "build.py",
        "harness": Path(__file__).resolve(),
        **{
            name: HERE / path
            for name, path in {
                "batch_harness": "batch_benchmark.py",
                "validator": "batch_validate.py",
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
    if args.imre_build_record is not None:
        inputs["imre_build_record"] = args.imre_build_record.resolve(strict=True)
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
        raise ValueError("saved Rust sources differ from the tracked batch reference")
    hashes = {name: shared.sha256(path) for name, path in inputs.items()}
    reference_record = batch.rust_record(inputs["rust_record"], rust_sources, hashes)
    thin_lto = args.lto_linker is not None
    existing.check_build_mode(reference_record, thin_lto)
    baseline = json.loads(inputs["baseline_record"].read_text())
    pre_exception = json.loads(inputs["pre_exception_record"].read_text())
    if (
        pre_exception.get("example") != "incremental_planner"
        or pre_exception.get("sources") != old_sources
        or pre_exception.get("sha256", {}).get("binary") != hashes["pre_exception"]
        or baseline.get("example") != "batch_planner"
        or baseline.get("sources") != sources
        or baseline.get("sha256", {}).get("control_binary")
        != hashes["baseline_control"]
        or baseline.get("sha256", {}).get("batch_binary") != hashes["baseline_batch"]
        or baseline.get("sha256", {}).get("baseline") != hashes["pre_exception"]
        or baseline.get("sha256", {}).get("baseline_record")
        != hashes["pre_exception_record"]
    ):
        raise ValueError(
            "frozen records must identify the exact programs, binaries, and pre-exception provenance"
        )
    for record in (pre_exception, baseline):
        if record.get("options", {}).get("lto") != (
            "thin" if thin_lto else None
        ) or record.get("options", {}).get("llvm_budget") != (
            1000 if thin_lto else None
        ):
            raise ValueError(
                "all saved Imre artifacts must match the selected LTO mode and budget"
            )
    prior_cases = {case["label"]: case for case in pre_exception["workloads"]}
    for case in baseline["workloads"]["control"]:
        prior = prior_cases[case["label"]]
        if any(
            case[key] != prior[key]
            for key in (
                "input",
                "expected_stdout",
                "input_sha256",
                "expected_stdout_sha256",
                "expected_stdout_bytes",
            )
        ):
            raise ValueError("control input or output changed between baseline records")
    protected = [
        *inputs.values(),
        *source_paths,
        *(source_root / name for name in rust_sources),
        *(REFERENCE / name for name in rust_sources),
        REFERENCE / "build.py",
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
        if thin_lto and original.get("runtime_lto_rustflags") != "-Clinker-plugin-lto":
            raise ValueError(
                "Imre build record must identify the linker-plugin-LTO runtime build"
            )
        runtime_name = "libimre_runtime_lto.a" if thin_lto else "libimre_runtime.a"
        if not any(
            Path(name).name == runtime_name and digest == hashes["runtime"]
            for name, digest in original["sha256"].items()
        ):
            raise ValueError("Imre build record identifies a different runtime mode")
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
    outputs = {
        name: artifacts / filename
        for name, filename in {
            "control_binary": "incremental_planner",
            "control_ir": "incremental_planner.ll",
            "batch_binary": "batch_planner",
            "batch_ir": "batch_planner.ll",
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
    if any(
        record["options"]["target"] != effective_target
        for record in (baseline, pre_exception)
    ):
        raise ValueError("baseline and current Clang effective targets differ")
    tools = {
        name: shared.run([str(inputs[key]), "--version"], b"", args.timeout)[
            "stdout"
        ].strip()
        for name, key in (("imre", "compiler"), ("clang", "clang"))
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
    for name, entry in (("control", incremental.ENTRY), ("batch", validator.ENTRY)):
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
            "pre_exception": [str(inputs["pre_exception"])],
            "baseline": [str(inputs["baseline_control"])],
            "imre": [str(outputs["control_binary"])],
        },
        "batch": {
            "cpython": validator.python_command(str(inputs["python"])),
            "baseline": [str(inputs["baseline_batch"])],
            "imre": [str(outputs["batch_binary"])],
            "rust_bigint": [str(inputs["rust_bigint"])],
        },
    }
    validation = {
        "control": incremental.validate_commands(commands["control"], args.timeout),
        "batch": validator.validate_commands(commands["batch"], args.timeout),
    }
    controls = [
        batch.captured_workload(case)
        for case in batch.select_workloads(
            {"workloads": baseline["workloads"]["control"]}, args.control_workloads
        )
    ]
    batches = captured_batches(baseline, args.batch_workloads)
    workloads = {
        group: [measure(workload, commands[group], args) for workload in cases]
        for group, cases in (("control", controls), ("batch", batches))
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
        "example": "exception_performance",
        "started_at_utc": started,
        "completed_at_utc": datetime.now(timezone.utc).isoformat(),
        "git": {"revision": revision, "dirty": dirty},
        "sources": sources,
        "rust_sources": rust_sources,
        "sha256": hashes,
        "artifacts": {
            name: {"bytes": path.stat().st_size}
            for name, path in inputs.items()
            if name
            in {
                *outputs,
                "pre_exception",
                "baseline_control",
                "baseline_batch",
                "rust_bigint",
            }
        },
        "rust_build_record": reference_record,
        "imre_build_provenance": imre_record,
        "baseline_provenance": saved_provenance(baseline, hashes["baseline_record"]),
        "pre_exception_provenance": saved_provenance(
            pre_exception, hashes["pre_exception_record"]
        ),
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
            "workloads": "exact captured previous control and batch inputs; no regeneration",
        },
        "method": {
            "timer": "time.perf_counter_ns around subprocess.run via benchmark.py",
            "includes": "fresh process startup, imports, pipes, parsing, graph validation, closure, scheduling, complete output and exit; batch also includes framing, recoverable errors, and cleanup",
            "excludes": "all builds, oracle/correctness gates, workload generation, warmups, output comparison and hashes",
            "order_cycles": {
                group: order_cycle(command) for group, command in commands.items()
            },
            "ordering": "four Williams orders repeat; each cycle puts every engine in every position once and every directed within-round adjacent pair once; cross-round predecessors are not balanced",
            "cpu": "harness and children inherit one CPU affinity; traced timing is refused",
            "output": "full input and expected stdout once per workload; every run exactly matches before retaining status/stderr/time/output SHA256 and length",
            "hash_seeds": "fresh processes retain default hash seeding; deterministic output never depends on set traversal order",
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
                    "pre_exception": ["<pre-exception-control-binary>"],
                    "baseline": ["<baseline-control-binary>"],
                    "imre": ["<control-binary>"],
                },
                "batch": {
                    "cpython": [
                        inputs["python"].name,
                        "-I",
                        "-c",
                        validator.BOOTSTRAP,
                        "<package-root>",
                    ],
                    "baseline": ["<baseline-batch-binary>"],
                    "imre": ["<batch-binary>"],
                    "rust_bigint": ["<rust-batch-bigint>"],
                },
            },
            "scope": "control compares identical source/input under pre-exception, post-exception, and optimized Imre; batch compares unchanged programs with prior/new Imre and Rust. Neither group isolates per-check or per-throw cost. Rust shares the original direct-port graph algorithm but uses native containers/indices and explicit Result recovery. No C or general language-speed inference.",
        },
        "validation": validation,
        "workloads": workloads,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    for option in (
        "compiler",
        "runtime",
        "imre-build-record",
        "pre-exception-control",
        "pre-exception-record",
        "baseline-control",
        "baseline-batch",
        "baseline-record",
        "rust-bigint",
        "rust-build-record",
    ):
        parser.add_argument("--" + option, type=Path, required=True)
    parser.add_argument("--rust-source-root", type=Path, default=REFERENCE)
    parser.add_argument("--python", default="python3.14")
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--target")
    parser.add_argument(
        "--artifact-dir", type=Path, default=HERE / "local/exception-planner"
    )
    parser.add_argument("--control-workload", action="append", dest="control_workloads")
    parser.add_argument("--batch-workload", action="append", dest="batch_workloads")
    parser.add_argument("--samples", type=shared.positive_int, default=32)
    parser.add_argument("--warmups", type=shared.positive_int, default=2)
    parser.add_argument("--cpu", type=int)
    parser.add_argument("--timeout", type=shared.positive_float, default=120.0)
    parser.add_argument(
        "--output", type=Path, default=HERE / "local/exception-comparison.json"
    )
    args = parser.parse_args()
    if args.samples % 4:
        parser.error(
            "--samples must be a multiple of four for balanced execution order"
        )
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
