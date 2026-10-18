"""Compare unchanged incremental, batch, and manifest programs across four engines.

The baseline bundle preserves binaries, their build records, exact captured inputs,
and the original Rust reference sources. All paths in its index are relative to
the index directory. New Relar artifacts are built once before correctness checks
and timing; existing baselines and Rust executables are never rebuilt here.
"""

import argparse
import itertools
import json
import os
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
manifest = SimpleNamespace(**runpy.run_path(str(HERE / "manifest_benchmark.py")))
exception = SimpleNamespace(**runpy.run_path(str(HERE / "exception_benchmark.py")))
shared, helpers = manifest.shared, manifest.helpers
VALIDATORS = {
    "incremental": manifest.incremental,
    "batch": manifest.validator.batch,
    "manifest": manifest.validator,
}
ENGINES = ("cpython", "baseline", "relar", "rust_bigint")


def runtime_context() -> dict:
    """Record relevant inherited settings without collecting unrelated secrets."""
    tracer = next(
        int(line.split(":", 1)[1])
        for line in Path("/proc/self/status").read_text().splitlines()
        if line.startswith("TracerPid:")
    )
    if tracer:
        raise RuntimeError("native benchmark/profile execution must be outside ptrace")
    names = (
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "PYTHONHASHSEED",
        "PYTHONMALLOC",
        "PYTHON_JIT",
        "PYTHON_GIL",
        "PYTHONUTF8",
        "MALLOC_ARENA_MAX",
        "MALLOC_PERTURB_",
        "GLIBC_TUNABLES",
        "LD_BIND_NOW",
        "LD_PRELOAD",
        "LD_LIBRARY_PATH",
        "ASAN_OPTIONS",
        "LSAN_OPTIONS",
        "UBSAN_OPTIONS",
        "RUST_BACKTRACE",
    )
    return {
        "tracer_pid": tracer,
        "inherited_environment": {name: os.environ.get(name) for name in names},
        "policy": "all engines inherit the same harness environment without per-engine overrides; CPython -I ignores Python environment settings; only this runtime-relevant allowlist is recorded",
    }


def build_provenance(record: dict) -> dict:
    """Keep build identity without repeating source text or old validation runs."""
    return {
        key: record[key]
        for key in (
            "git",
            "git_head",
            "git_status",
            "sources",
            "source_sha256",
            "source_snapshot_sha256",
            "compiled_sources_match_commit",
            "sha256",
            "tools",
            "options",
            "compilation",
            "compile_command",
            "compile_elapsed_ns",
            "relar_build_provenance",
            "rustc_verbose",
            "runtime_lto_rustflags",
        )
        if key in record
    }


def load_baseline(path: Path, thin_lto: bool, inputs: dict, hashes: dict) -> dict:
    """Verify the sealed baseline bundle and register every protected input."""
    path = path.resolve(strict=True)
    root = path.parent
    index = json.loads(path.read_text())
    inputs["baseline_index"] = path
    hashes["baseline_index"] = shared.sha256(path)
    if index.get("schema_version") != 1 or index.get("git", {}).get("dirty"):
        raise ValueError("baseline index must identify a clean source snapshot")

    def register(name: str, spec: dict) -> Path:
        relative = Path(spec["path"])
        selected = (root / relative).resolve(strict=True)
        if relative.is_absolute() or not selected.is_relative_to(root):
            raise ValueError("baseline paths must remain inside the frozen bundle")
        digest = shared.sha256(selected)
        if digest != spec["sha256"] or selected.stat().st_size != spec["bytes"]:
            raise ValueError(f"frozen baseline identity changed: {name}")
        inputs["baseline:" + name] = selected
        hashes["baseline:" + name] = digest
        return selected

    register("preservation", index["preservation"])
    register("source_snapshot", index["source_snapshot"])
    tools = {name: register(name, spec) for name, spec in index["tools"].items()}
    tool_record = json.loads(tools["build-record.json"].read_text())
    if not tool_record.get("source_sha256") or any(
        index["source_sha256"].get(name) != digest
        for name, digest in tool_record["source_sha256"].items()
    ):
        raise ValueError("frozen compiler/runtime sources differ from their snapshot")
    for name in ("relar", "librelar_runtime.a", "librelar_runtime_lto.a"):
        if not any(
            Path(key).name == name and digest == shared.sha256(tools[name])
            for key, digest in tool_record["sha256"].items()
        ):
            raise ValueError(f"frozen build record does not identify {name}")
    if thin_lto and tool_record.get("runtime_lto_rustflags") != "-Clinker-plugin-lto":
        raise ValueError("frozen runtime record does not identify its bitcode build")
    mode = index["modes"]["lto" if thin_lto else "normal"]
    expected_mode = {
        "lto": "thin" if thin_lto else None,
        "llvm_budget": 1000 if thin_lto else None,
        "optimization_level": 3,
    }
    if any(mode["options"].get(key) != value for key, value in expected_mode.items()):
        raise ValueError("baseline linking mode or optimization budget differs")
    records = {
        name: json.loads(register(name, mode[name]).read_text())
        for name in ("manifest_record", "batch_input_record", "batch_build_record")
    }
    old_manifest, old_batch = records["manifest_record"], records["batch_input_record"]
    batch_build = records["batch_build_record"]
    if (
        old_manifest.get("example") != "manifest_planner"
        or old_batch.get("example") != "exception_performance"
    ):
        raise ValueError("captured input records identify unexpected programs")
    if old_manifest["sources"] != index["sources"]:
        raise ValueError("manifest baseline source identity changed")
    batch_sources = {
        name: digest
        for name, digest in index["sources"].items()
        if not name.startswith("manifest_planner/")
    }
    if old_batch["sources"] != batch_sources or batch_build["sources"] != batch_sources:
        raise ValueError("batch baseline source identity changed")
    runtime_name = "librelar_runtime_lto.a" if thin_lto else "librelar_runtime.a"
    for record in (old_manifest, batch_build):
        if any(
            record["options"].get(key) != value
            for key, value in mode["options"].items()
        ):
            raise ValueError("baseline program modes/targets disagree")
        if record["sha256"]["compiler"] != shared.sha256(tools["relar"]) or record[
            "sha256"
        ]["runtime"] != shared.sha256(tools[runtime_name]):
            raise ValueError("baseline programs do not use the same frozen tools")
        for tool in ("clang", "lto_linker") if thin_lto else ("clang",):
            if tool in hashes and record["sha256"].get(tool) != hashes[tool]:
                raise ValueError(f"baseline and new programs use different {tool}")
    binaries, references, reference_records = {}, {}, {}
    for group, specs in mode["programs"].items():
        if group not in VALIDATORS:
            raise ValueError(f"unknown baseline program: {group}")
        for kind in ("binary", "ir"):
            selected = register(group + "_" + kind, specs[kind])
            record = batch_build if group == "batch" else old_manifest
            key = (
                kind
                if group == "batch"
                else ("control" if group == "incremental" else group) + "_" + kind
            )
            if shared.sha256(selected) != record["sha256"][key]:
                raise ValueError(f"baseline {group} {kind} differs from build evidence")
            if kind == "binary":
                binaries[group] = selected
        reference = mode["references"][group]
        references[group] = register("rust_" + group, reference["binary"])
        record_path = register("rust_record_" + group, reference["build_record"])
        reference_record = json.loads(record_path.read_text())
        if reference_record.get("entrypoint", "incremental") != group:
            raise ValueError("Rust reference entrypoint differs from its group")
        if reference_record["binaries"]["bigint"]["sha256"] != shared.sha256(
            references[group]
        ):
            raise ValueError("Rust reference binary differs from its build record")
        manifest.existing.check_build_mode(reference_record, thin_lto)
        source_root = root / reference["source_root"]
        sources = reference_record["sources"]
        if helpers.rust_sources(source_root) != sources or set(
            reference_record["source_contents"]
        ) != set(sources):
            raise ValueError("preserved Rust reference source set changed")
        for name, digest in sources.items():
            if manifest.text_hash(reference_record["source_contents"][name]) != digest:
                raise ValueError("Rust source text differs from its recorded hash")
            source = source_root / name
            register(
                "rust_source_" + group + ":" + name,
                {
                    "path": str(source.relative_to(root)),
                    "sha256": digest,
                    "bytes": source.stat().st_size,
                },
            )
        script = source_root / "build.py"
        script_hash = reference_record["build_script_sha256"]
        if manifest.text_hash(reference_record["build_script"]) != script_hash:
            raise ValueError("Rust build helper differs from its recorded hash")
        register(
            "rust_build_script_" + group,
            {
                "path": str(script.relative_to(root)),
                "sha256": script_hash,
                "bytes": script.stat().st_size,
            },
        )
        if not reference_record.get("rustc_verbose") or not reference_record.get(
            "cargo_version"
        ):
            raise ValueError("Rust reference toolchain provenance is missing")
        reference_records[group] = reference_record
    if set(binaries) != set(VALIDATORS):
        raise ValueError("baseline must preserve all three programs")
    return {
        "index": index,
        "mode": mode,
        "binaries": binaries,
        "references": references,
        "reference_records": reference_records,
        "records": records,
        "tool_record": tool_record,
    }


def captured_manifests(record: dict, labels: list[str] | None) -> list[dict]:
    """Check captured filesystem bytes against the independent application oracle."""
    validator = VALIDATORS["manifest"]
    result = []
    for workload in manifest.select_workloads(
        {"workloads": record["workloads"]["manifest"]}, labels
    ):
        case = validator.case_from_record(workload["case"])
        expected = validator.oracle(case)
        inventory = {
            name: {"sha256": validator.digest(data), "bytes": len(data)}
            for name, data in sorted(expected["files"].items())
        }
        changed = validator.encoded_files(
            {
                name: data
                for name, data in expected["files"].items()
                if case.files.get(name) != data
            }
        )
        if (
            expected["returncode"] != 0
            or expected["stdout"] != workload["expected_stdout"]
            or inventory != workload["expected_file_inventory"]
            or changed != workload["expected_changed_files"]
            or validator.case_record(case) != workload["case"]
        ):
            raise ValueError("captured manifest differs from the independent oracle")
        for captured, field in (
            ("requests", "requests"),
            ("completed_requests", "completed"),
            ("invalid_requests", "invalid"),
            ("cleaned_requests", "cleaned"),
        ):
            if workload[captured] != expected[field]:
                raise ValueError("captured manifest request accounting differs")
        result.append(
            {
                key: value
                for key, value in workload.items()
                if key not in {"warmups", "samples", "summary"}
            }
        )
    return result


def measure_manifest(workload: dict, commands: dict, args: argparse.Namespace) -> dict:
    """Use the same filesystem timer boundary with four Williams orders."""
    validator = VALIDATORS["manifest"]
    case = validator.case_from_record(workload["case"])
    expected = validator.oracle(case)
    sandboxes = args.artifact_dir.resolve() / "sandboxes"
    sandboxes.mkdir(parents=True, exist_ok=True)

    def run(name: str) -> dict:
        with tempfile.TemporaryDirectory(prefix=name + "-", dir=sandboxes) as temporary:
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
    phases = {"warmups": [], "samples": []}
    orders = exception.order_cycle(commands)
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
    execution_before = runtime_context()
    thin_lto = args.lto_linker is not None
    artifacts = args.artifact_dir.resolve()
    inputs = {
        "compiler": Path(shared.executable(str(args.compiler))),
        "runtime": args.runtime.resolve(strict=True),
        "relar_build_record": args.relar_build_record.resolve(strict=True),
        "clang": Path(shared.executable(args.clang)),
        "harness": Path(__file__).resolve(),
        **{
            name: HERE / name
            for name in (
                "manifest_benchmark.py",
                "manifest_validate.py",
                "exception_benchmark.py",
                "batch_benchmark.py",
                "batch_validate.py",
                "incremental_benchmark.py",
                "incremental_validate.py",
                "dependency_compare.py",
                "dependency_benchmark.py",
                "dependency_validate.py",
                "benchmark.py",
                "validate.py",
                "reference/dependency_planner/check_boundaries.py",
            )
        },
    }
    flags, recorded_flags = [], []
    if thin_lto:
        # Preserve ld.lld argv[0] flavor; resolve identity only in guards/hashes.
        inputs["lto_linker"] = args.lto_linker.absolute()
        flags += ["--lto-linker", str(inputs["lto_linker"])]
        recorded_flags += ["--lto-linker", "<linker-dir>/" + inputs["lto_linker"].name]
    if args.target is not None:
        flags += ["--target", args.target]
        recorded_flags += ["--target", args.target]
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
    hashes = {name: shared.sha256(path) for name, path in inputs.items()}
    baseline = load_baseline(args.baseline_record, thin_lto, inputs, hashes)
    source_paths = VALIDATORS["manifest"].package_sources()
    sources = {
        str(path.relative_to(VALIDATORS["manifest"].PACKAGE_ROOT)): shared.sha256(path)
        for path in source_paths
    }
    if sources != baseline["index"]["sources"]:
        raise ValueError("Python programs changed from the frozen baseline")
    relar_record = json.loads(inputs["relar_build_record"].read_text())
    runtime_name = "librelar_runtime_lto.a" if thin_lto else "librelar_runtime.a"
    for name, digest in (
        ("relar", hashes["compiler"]),
        (runtime_name, hashes["runtime"]),
    ):
        if not any(
            Path(key).name == name and value == digest
            for key, value in relar_record.get("sha256", {}).items()
        ):
            raise ValueError(f"Relar build record does not identify selected {name}")
    if thin_lto and relar_record.get("runtime_lto_rustflags") != "-Clinker-plugin-lto":
        raise ValueError("Relar record must identify the bitcode runtime build")
    if not relar_record.get("source_sha256"):
        raise ValueError("Relar build record must preserve source hashes")
    for name, digest in relar_record["source_sha256"].items():
        path = ROOT / name
        if shared.sha256(path) != digest:
            raise ValueError(f"Relar build source changed: {name}")
        inputs["relar_source:" + name], hashes["relar_source:" + name] = path, digest
    outputs = {
        group + "_" + kind: artifacts
        / (group + "_planner" + (".ll" if kind == "ir" else ""))
        for group in VALIDATORS
        for kind in ("binary", "ir")
    }
    helpers.boundaries.protect_outputs(
        [
            *outputs.values(),
            args.output,
            args.output.with_name(args.output.name + ".tmp"),
        ],
        [*inputs.values(), *source_paths],
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
    if baseline["mode"]["options"]["target"] != effective_target:
        raise ValueError("baseline and new Clang effective targets differ")
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
    old_manifest = baseline["records"]["manifest_record"]
    captured = {
        "incremental": [
            manifest.captured_workload(case)
            for case in manifest.select_workloads(
                {"workloads": old_manifest["workloads"]["control"]},
                args.incremental_workloads,
            )
        ],
        "batch": exception.captured_batches(
            baseline["records"]["batch_input_record"], args.batch_workloads
        ),
        "manifest": captured_manifests(old_manifest, args.manifest_workloads),
    }
    artifacts.mkdir(parents=True, exist_ok=True)
    compilations, compile_commands, commands = {}, {}, {}
    for group, validator in VALIDATORS.items():
        binary, llvm = outputs[group + "_binary"], outputs[group + "_ir"]
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
        compilations[group] = shared.run(command, b"", args.timeout)
        compile_commands[group] = [
            inputs["compiler"].name,
            "build",
            str(validator.ENTRY.relative_to(ROOT)),
            "--module-root",
            "examples",
            "-o",
            "<artifact-dir>/" + binary.name,
            "--runtime",
            "<runtime>",
            "--clang",
            inputs["clang"].name,
            "--opt-level",
            "3",
            "--emit-llvm",
            "<artifact-dir>/" + llvm.name,
            *recorded_flags,
        ]
        commands[group] = {
            "cpython": validator.python_command(str(inputs["python"])),
            "baseline": [str(baseline["binaries"][group])],
            "relar": [str(binary)],
            "rust_bigint": [str(baseline["references"][group])],
        }
    inputs.update(outputs)
    hashes.update({name: shared.sha256(path) for name, path in outputs.items()})
    validation = {
        group: validator.validate_commands(commands[group], args.timeout)
        for group, validator in VALIDATORS.items()
    }
    execution_samples = runtime_context()
    workloads = (
        captured
        if args.validate_only
        else {
            group: [
                (measure_manifest if group == "manifest" else exception.measure)(
                    workload, commands[group], args
                )
                for workload in cases
            ]
            for group, cases in captured.items()
        }
    )
    if VALIDATORS["manifest"].package_sources() != source_paths:
        raise RuntimeError("Python source set changed during comparison")
    for path in source_paths:
        if (
            shared.sha256(path)
            != sources[str(path.relative_to(VALIDATORS["manifest"].PACKAGE_ROOT))]
        ):
            raise RuntimeError("Python source changed during comparison")
    for name, path in inputs.items():
        if shared.sha256(path) != hashes[name]:
            raise RuntimeError(f"{name} changed during comparison")
    execution_after = runtime_context()
    if execution_before != execution_samples or execution_before != execution_after:
        raise RuntimeError("recorded runtime environment changed during comparison")
    return {
        "schema_version": 1,
        "example": "concrete_layouts",
        "started_at_utc": started,
        "completed_at_utc": datetime.now(timezone.utc).isoformat(),
        "git": {"revision": revision, "dirty": dirty},
        "sources": sources,
        "sha256": hashes,
        "artifacts": {
            name: {"bytes": path.stat().st_size}
            for name, path in inputs.items()
            if name in outputs
            or name.startswith(
                (
                    "baseline:rust_",
                    "baseline:incremental_",
                    "baseline:batch_",
                    "baseline:manifest_",
                )
            )
        },
        "baseline_provenance": {
            "index": baseline["index"],
            "tool_build": build_provenance(baseline["tool_record"]),
            "program_builds": {
                name: build_provenance(record)
                for name, record in baseline["records"].items()
            },
        },
        "rust_build_records": baseline["reference_records"],
        "relar_build_provenance": build_provenance(relar_record),
        "tools": tools,
        "execution": {
            "before": execution_before,
            "before_samples": execution_samples,
            "after": execution_after,
        },
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
            "validation_only": args.validate_only,
        },
        "method": {
            "timer": "time.perf_counter_ns around subprocess.run; explicit fresh cwd for manifest workloads",
            "includes": "fresh process startup, imports, parsing, graph validation, affected closure, scheduling, complete output and exit; batch includes framing/recovery/cleanup; manifest includes file open/read/write/flush/close",
            "excludes": "all builds, independent correctness checks, fixture creation/deletion, warmups, full-output/filesystem comparisons and hashes",
            "order_cycles": {
                group: exception.order_cycle(command)
                for group, command in commands.items()
            },
            "ordering": "four Williams orders repeat; each cycle puts every engine in every position once and every directed within-round adjacent pair once; cross-round predecessors are not balanced",
            "cpu": "harness and children inherit one CPU affinity; traced timing is refused",
            "output": "full captured input and expected output once per workload; every run checks complete stdout/stderr/status; manifests also check all directory bytes and retain full expected changed files plus inventory hashes",
            "filesystem": "each manifest execution gets a fresh isolated cwd; fixtures created immediately before timing are page-cache warm; outputs are flushed/closed without fsync; no cold-disk or durability claim",
            "hash_seeds": "fresh processes retain default hash seeding; deterministic output never depends on set traversal order",
            "compile_commands": compile_commands,
            "commands": {
                group: {
                    "cpython": [
                        inputs["python"].name,
                        "-I",
                        "-c",
                        validator.BOOTSTRAP,
                        "<package-root>",
                    ],
                    "baseline": [f"<baseline-{group}>"],
                    "relar": [f"<relar-{group}>"],
                    "rust_bigint": [f"<rust-{group}-bigint>"],
                }
                for group, validator in VALIDATORS.items()
            },
            "manifest_arguments": "append the exact captured case.arguments to each engine command",
            "scope": "each group compares unchanged Python source/input under prior/new Relar and a preserved arbitrary-precision Rust direct port; original reference snapshots/build records are used without rewriting or rebuilding their algorithms; these are whole-program workload results, not isolated layout costs or general language-speed estimates",
        },
        "validation": validation,
        "workloads": workloads,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("compiler", "runtime", "relar-build-record", "baseline-record"):
        parser.add_argument("--" + option, type=Path, required=True)
    parser.add_argument("--python", default="python3.14")
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--target")
    parser.add_argument(
        "--artifact-dir", type=Path, default=HERE / "local/layout-comparison"
    )
    for group in VALIDATORS:
        parser.add_argument(
            "--" + group + "-workload", action="append", dest=group + "_workloads"
        )
    parser.add_argument("--samples", type=shared.positive_int, default=32)
    parser.add_argument("--warmups", type=shared.positive_int, default=2)
    parser.add_argument("--cpu", type=int)
    parser.add_argument("--timeout", type=shared.positive_float, default=120.0)
    parser.add_argument(
        "--validate-only",
        action="store_true",
        help="compile and run all independent oracles without benchmark samples",
    )
    parser.add_argument(
        "--output", type=Path, default=HERE / "local/layout-comparison.json"
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
