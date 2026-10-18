"""Whole-process inventory comparisons with captured files and optional old-source controls."""

import argparse
import itertools
import json
import math
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
REFERENCE = HERE / "reference/inventory_reconciliation"
prior = SimpleNamespace(**runpy.run_path(str(HERE / "layout_benchmark.py")))
validator = SimpleNamespace(**runpy.run_path(str(HERE / "inventory_validate.py")))
shared, helpers = prior.shared, prior.helpers
incremental = prior.manifest.incremental


def corpus() -> list:
    result = [
        validator.case("empty", [], []),
        validator.case(
            "boundary-128",
            [("café", 10**80), ("部品", 2**63), ("café", 10**80 + 7), ("untouched", 3)],
            [
                [
                    (
                        "café"
                        if index % 3 == 0
                        else "部品"
                        if index % 3 == 1
                        else "unknown",
                        10**79 + index,
                    )
                    for index in range(128)
                ],
                [],
                [("café", 0), ("部品", -2)],
            ],
        ),
    ]
    for size in (2048, 16384):
        for shape in ("dense", "sparse"):
            unique = 64 if shape == "dense" else size
            names = [
                ("部品-" if index % 11 == 0 else "SKU-") + str(index)
                for index in range(unique)
            ]
            stocks = [
                (name, size // unique * 5 if shape == "dense" else 5 + index % 13)
                for index, name in enumerate(names)
            ]
            stocks.extend(
                (names[index], stocks[index][1] + 3) for index in range(0, unique, 64)
            )
            batches = []
            for start in range(0, size, 256):
                batch = []
                for index in range(start, min(start + 256, size)):
                    sku = names[(index * 7) % unique]
                    if shape == "sparse" and index % 5 == 0:
                        sku = "unknown-" + str(index)
                    batch.append((sku, 1 + index % 17))
                    if index % 13 == 0:
                        batch.extend(((sku, 0), (sku, -3)))
                batches.append(batch)
                if len(batches) % 4 == 0:
                    batches.append([])
            result.append(validator.case(f"{shape}-{size}", stocks, batches))
    return result


def workload(value) -> dict:
    expected = validator.oracle(value)
    if expected["returncode"] != 0:
        raise ValueError("timed inventory workloads must be valid")
    stocks = validator.parse(value.files["stock.txt"].decode(), "stock")
    batches = validator.parse(value.files["orders.txt"].decode(), "orders")
    return {
        "label": value.label,
        "case": validator.case_record(value),
        "stock_rows": len(stocks),
        "stock_skus": len(dict(stocks)),
        "batches": len(batches),
        "request_rows": sum(map(len, batches)),
        "positive_requests": sum(
            quantity > 0 for batch in batches for _, quantity in batch
        ),
        "expected_stdout": expected["stdout"],
        "expected_changed_files": validator.encoded_files(
            {
                name: data
                for name, data in expected["files"].items()
                if value.files.get(name) != data
            }
        ),
        "expected_file_inventory": {
            name: {"sha256": validator.digest(data), "bytes": len(data)}
            for name, data in sorted(expected["files"].items())
        },
    }


def captured_workloads(record: dict) -> list[dict]:
    """Keep a comparison's exact input bytes, independently checking its oracle data."""
    result = []
    labels = set()
    for captured in record["workloads"]["inventory"]:
        case = validator.case_from_record(captured["case"])
        for name in (
            *case.arguments,
            *case.files,
            *case.directories,
            *case.hardlinks,
            *case.hardlinks.values(),
        ):
            if Path(name).is_absolute() or ".." in Path(name).parts or "\0" in name:
                raise ValueError("captured workload paths must stay inside the fixture")
        checked = workload(case)
        if case.label in labels or any(
            captured.get(key) != value for key, value in checked.items()
        ):
            raise ValueError("captured workload differs from independent oracle")
        labels.add(case.label)
        result.append(checked)
    if not result:
        raise ValueError("inventory baseline must contain captured workloads")
    return result


def measure(value: dict, commands: dict, args) -> dict:
    case = validator.case_from_record(value["case"])
    expected = validator.oracle(case)
    assert value == workload(case), "captured workload differs from independent oracle"
    sandbox = args.artifact_dir.resolve() / "sandboxes"
    sandbox.mkdir(parents=True, exist_ok=True)

    def run(engine: str) -> dict:
        with tempfile.TemporaryDirectory(prefix=engine + "-", dir=sandbox) as temporary:
            directory = Path(temporary)
            validator.materialize(case, directory)
            start = time.perf_counter_ns()
            process = subprocess.run(
                [*commands[engine], *case.arguments],
                input=b"",
                capture_output=True,
                cwd=directory,
                timeout=args.timeout,
                check=False,
            )
            elapsed = time.perf_counter_ns() - start
            return {
                "elapsed_ns": elapsed,
                **validator.check_result(case, expected, process, directory),
            }

    gates = {
        name: {key: value for key, value in run(name).items() if key != "elapsed_ns"}
        for name in commands
    }
    phases = {"warmups": [], "samples": []}
    orders = tuple(itertools.permutations(commands))
    if not args.validate_only:
        for phase, count in (("warmups", args.warmups), ("samples", args.samples)):
            for index in range(count):
                order = orders[index % len(orders)]
                sample = {"index": index, "order": order}
                for name in order:
                    sample[name] = run(name)
                phases[phase].append(sample)
    summaries = {}
    if phases["samples"]:
        summaries = {name: shared.summary(phases["samples"], name) for name in commands}
        medians = {name: item["median_seconds"] for name, item in summaries.items()}
        summaries["ratios_of_medians"] = {
            f"{left}_over_{right}": medians[left] / medians[right]
            for left, right in itertools.permutations(commands, 2)
        }
        print(
            value["label"]
            + ": "
            + ", ".join(
                f"{name} {seconds * 1000:.3f}ms" for name, seconds in medians.items()
            ),
            flush=True,
        )
    return {**value, "gates": gates, **phases, "summary": summaries}


def rust_record(path: Path, sources: dict, hashes: dict) -> dict:
    record = json.loads(path.read_text())
    contents = record.get("source_contents", {})
    if (
        record.get("entrypoint") != "inventory"
        or record.get("sources") != sources
        or set(contents) != set(sources)
        or any(
            validator.digest(contents[name].encode()) != digest
            for name, digest in sources.items()
        )
    ):
        raise ValueError(
            "Rust build record must preserve the exact inventory reference sources"
        )
    if (
        record.get("binaries", {}).get("bigint", {}).get("sha256")
        != hashes["rust_bigint"]
    ):
        raise ValueError("Rust build record identifies another binary")
    if (
        record.get("build_script_sha256") != hashes["rust_build_script"]
        or validator.digest(record.get("build_script", "").encode())
        != hashes["rust_build_script"]
    ):
        raise ValueError("Rust build helper differs from its preserved source")
    if not record.get("rustc_verbose") or not record.get("cargo_version"):
        raise ValueError("Rust tool versions are required")
    return record


def benchmark(args) -> dict:
    started = datetime.now(timezone.utc).isoformat()
    cpu = shared.pin_cpu(args.cpu)
    execution = {"before": prior.runtime_context()}
    thin = args.lto_linker is not None
    artifacts = args.artifact_dir.resolve()
    source_root = args.rust_source_root.resolve(strict=True)
    inputs = {
        "compiler": Path(shared.executable(str(args.compiler))),
        "runtime": args.runtime.resolve(strict=True),
        "relar_build_record": args.relar_build_record.resolve(strict=True),
        "clang": Path(shared.executable(args.clang)),
        "rust_bigint": Path(shared.executable(str(args.rust_bigint))),
        "rust_record": args.rust_build_record.resolve(strict=True),
        "rust_build_script": source_root / "build.py",
        "tracked_rust_build_script": REFERENCE / "build.py",
        **{"harness:" + path.name: path for path in sorted(HERE.glob("*.py"))},
        "guard_helpers": HERE / "reference/dependency_planner/check_boundaries.py",
    }
    flags = []
    if thin:
        inputs["lto_linker"] = args.lto_linker.absolute()
        flags += ["--lto-linker", str(inputs["lto_linker"])]
    if args.target:
        flags += ["--target", args.target]
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
    if args.baseline:
        inputs["baseline"] = Path(shared.executable(str(args.baseline)))
        inputs["baseline_record"] = args.baseline_record.resolve(strict=True)
    if args.inventory_baseline:
        inputs["inventory_baseline"] = Path(
            shared.executable(str(args.inventory_baseline))
        )
        inputs["inventory_baseline_record"] = args.inventory_baseline_record.resolve(
            strict=True
        )
    source_paths = validator.package_sources() + (
        incremental.package_sources() if args.baseline else []
    )
    source_hashes = {
        str(path.relative_to(validator.PACKAGE_ROOT)): shared.sha256(path)
        for path in source_paths
    }
    rust_sources = helpers.rust_sources(source_root)
    if rust_sources != helpers.rust_sources(REFERENCE) or shared.sha256(
        source_root / "build.py"
    ) != shared.sha256(REFERENCE / "build.py"):
        raise ValueError(
            "selected Rust source snapshot must match the tracked reference"
        )
    hashes = {name: shared.sha256(path) for name, path in inputs.items()}
    reference = rust_record(inputs["rust_record"], rust_sources, hashes)
    prior.manifest.existing.check_build_mode(reference, thin)
    build = json.loads(inputs["relar_build_record"].read_text())
    if not {hashes["compiler"], hashes["runtime"]} <= set(
        build.get("sha256", {}).values()
    ):
        raise ValueError(
            "Relar build record does not identify selected compiler/runtime"
        )
    runtime_name = "librelar_runtime_lto.a" if thin else "librelar_runtime.a"
    if not any(
        Path(name).name == runtime_name and digest == hashes["runtime"]
        for name, digest in build["sha256"].items()
    ):
        raise ValueError("runtime archive does not match the recorded linking mode")
    if thin and build.get("runtime_lto_rustflags") != "-Clinker-plugin-lto":
        raise ValueError("ThinLTO runtime must record -Clinker-plugin-lto")
    if not build.get("source_sha256"):
        raise ValueError("Relar source hashes are required")
    for name, digest in build["source_sha256"].items():
        path = ROOT / name
        if shared.sha256(path) != digest:
            raise ValueError("Relar source differs from frozen build: " + name)
        inputs["relar_source:" + name], hashes["relar_source:" + name] = path, digest
    target_flags = ["--target=" + args.target] if args.target else []
    target = shared.run(
        [str(inputs["clang"]), *target_flags, "-dumpmachine"], b"", args.timeout
    )["stdout"].strip()
    rust_host = next(
        (
            line.removeprefix("host: ")
            for line in reference["rustc_verbose"].splitlines()
            if line.startswith("host: ")
        ),
        None,
    )
    if rust_host != target:
        raise ValueError("Rust host target and effective Clang target must match")
    baseline = None
    if args.baseline:
        baseline = json.loads(inputs["baseline_record"].read_text())
        old_sources = {
            name: digest
            for name, digest in source_hashes.items()
            if name.startswith("incremental_planner/")
        }
        if (
            baseline.get("example") != "concrete_layouts"
            or baseline.get("sha256", {}).get("incremental_binary")
            != hashes["baseline"]
            or {
                name: digest
                for name, digest in baseline.get("sources", {}).items()
                if name.startswith("incremental_planner/")
            }
            != old_sources
        ):
            raise ValueError(
                "baseline must identify unchanged incremental source and exact saved binary"
            )
        if (
            baseline["options"].get("lto") != ("thin" if thin else None)
            or baseline["options"].get("llvm_budget") != (1000 if thin else None)
            or baseline["options"].get("target") != target
        ):
            raise ValueError("baseline target/linking mode/LLVM budgets differ")
        for name in ("clang", "lto_linker"):
            if name in inputs and baseline["sha256"].get(name) != hashes[name]:
                raise ValueError("baseline and current tool differ: " + name)
    inventory_baseline = None
    inventory_baseline_source = None
    inventory_baseline_paths = []
    inventory_baseline_root = (
        args.inventory_baseline_source_root.resolve(strict=True)
        if args.inventory_baseline_source_root
        else None
    )
    inventory_workloads = None
    if args.inventory_baseline:
        inventory_baseline = json.loads(inputs["inventory_baseline_record"].read_text())
        inventory_sources = {
            name: digest
            for name, digest in source_hashes.items()
            if name.startswith("inventory_reconciliation/")
        }
        if (
            inventory_baseline.get("example") != "inventory_reconciliation"
            or inventory_baseline.get("sha256", {}).get("inventory_binary")
            != hashes["inventory_baseline"]
        ):
            raise ValueError("inventory baseline must identify the exact saved binary")
        recorded_sources = {
            name: digest
            for name, digest in inventory_baseline.get("sources", {}).items()
            if name.startswith("inventory_reconciliation/")
        }
        if inventory_baseline_root:
            inventory_baseline_paths = sorted(
                (inventory_baseline_root / "inventory_reconciliation").rglob("*.py")
            )
            snapshot_sources = {
                str(path.relative_to(inventory_baseline_root)): shared.sha256(path)
                for path in inventory_baseline_paths
            }
            if not recorded_sources or snapshot_sources != recorded_sources:
                raise ValueError(
                    "inventory baseline source snapshot must match the complete recorded package"
                )
            inventory_baseline_source = {
                "snapshot_root": str(inventory_baseline_root),
                "recorded_sources": recorded_sources,
                "current_sources": inventory_sources,
                "changed_sources": {
                    name: {
                        "baseline_sha256": recorded_sources.get(name),
                        "current_sha256": inventory_sources.get(name),
                    }
                    for name in sorted(
                        recorded_sources.keys() | inventory_sources.keys()
                    )
                    if recorded_sources.get(name) != inventory_sources.get(name)
                },
            }
            for path in inventory_baseline_paths:
                name = str(path.relative_to(inventory_baseline_root))
                key = "inventory_baseline_source:" + name
                inputs[key], hashes[key] = path, snapshot_sources[name]
        elif recorded_sources != inventory_sources:
            raise ValueError(
                "inventory baseline must identify unchanged source unless a verified source snapshot is supplied"
            )
        if (
            inventory_baseline["options"].get("lto") != ("thin" if thin else None)
            or inventory_baseline["options"].get("llvm_budget")
            != (1000 if thin else None)
            or inventory_baseline["options"].get("target") != target
        ):
            raise ValueError(
                "inventory baseline target/linking mode/LLVM budgets differ"
            )
        for name in ("clang", "lto_linker"):
            if (
                name in inputs
                and inventory_baseline["sha256"].get(name) != hashes[name]
            ):
                raise ValueError("inventory baseline and current tool differ: " + name)
        inventory_workloads = captured_workloads(inventory_baseline)
    outputs = {
        "inventory_binary": artifacts / "inventory_reconciliation",
        "inventory_ir": artifacts / "inventory_reconciliation.ll",
    }
    if baseline:
        outputs.update(
            control_binary=artifacts / "incremental_planner",
            control_ir=artifacts / "incremental_planner.ll",
        )
    protected = [
        *inputs.values(),
        *source_paths,
        *(source_root / name for name in rust_sources),
        *(REFERENCE / name for name in rust_sources),
        REFERENCE / "build.py",
    ]
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
        if thin
        else None
    )
    tools.update(cpython=python_info, harness_python=sys.version)
    artifacts.mkdir(parents=True, exist_ok=True)
    compilations, compile_commands = {}, {}
    entries = {
        "inventory": validator.ENTRY,
        **({"control": incremental.ENTRY} if baseline else {}),
    }
    for name, entry in entries.items():
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
        compile_commands[name] = command
    inputs.update(outputs)
    hashes.update({name: shared.sha256(path) for name, path in outputs.items()})
    commands = {
        "inventory": {
            "cpython": validator.python_command(str(inputs["python"])),
            "relar": [str(outputs["inventory_binary"])],
            "rust_bigint": [str(inputs["rust_bigint"])],
        }
    }
    if inventory_baseline:
        commands["inventory"]["baseline"] = [str(inputs["inventory_baseline"])]
    validation = {
        "inventory": validator.validate_commands(commands["inventory"], args.timeout)
    }
    controls = []
    if baseline:
        commands["control"] = {
            "cpython": incremental.python_command(str(inputs["python"])),
            "baseline": [str(inputs["baseline"])],
            "relar": [str(outputs["control_binary"])],
        }
        validation["control"] = incremental.validate_commands(
            commands["control"], args.timeout
        )
        controls = [
            prior.manifest.captured_workload(value)
            for value in prior.manifest.select_workloads(
                {"workloads": baseline["workloads"]["incremental"]},
                args.control_workloads,
            )
        ]
    selected = prior.manifest.select_workloads(
        {"workloads": inventory_workloads or [workload(value) for value in corpus()]},
        args.workloads,
    )
    execution["before_samples"] = prior.runtime_context()
    measured_controls = []
    for value in controls:
        if args.validate_only:
            for command in commands["control"].values():
                assert (
                    shared.run(command, value["input"].encode(), args.timeout)["stdout"]
                    == value["expected_stdout"]
                )
            measured_controls.append(
                {**value, "warmups": [], "samples": [], "summary": {}}
            )
        else:
            measured_controls.append(
                prior.manifest.measure_control(value, commands["control"], args)
            )
    workloads = {
        "control": measured_controls,
        "inventory": [
            measure(value, commands["inventory"], args) for value in selected
        ],
    }
    execution["after"] = prior.runtime_context()
    current_paths = validator.package_sources() + (
        incremental.package_sources() if baseline else []
    )
    if (
        current_paths != source_paths
        or helpers.rust_sources(source_root) != rust_sources
        or helpers.rust_sources(REFERENCE) != rust_sources
        or inventory_baseline_root
        and sorted((inventory_baseline_root / "inventory_reconciliation").rglob("*.py"))
        != inventory_baseline_paths
    ):
        raise RuntimeError("source set changed during comparison")
    for path in source_paths:
        if (
            shared.sha256(path)
            != source_hashes[str(path.relative_to(validator.PACKAGE_ROOT))]
        ):
            raise RuntimeError("Python source changed during comparison")
    for name, path in inputs.items():
        if shared.sha256(path) != hashes[name]:
            raise RuntimeError("input/tool/artifact changed during comparison: " + name)
    return {
        "schema_version": 1,
        "example": "inventory_reconciliation",
        "started_at_utc": started,
        "completed_at_utc": datetime.now(timezone.utc).isoformat(),
        "git": {"revision": revision, "dirty": dirty},
        "sources": source_hashes,
        "rust_sources": rust_sources,
        "sha256": hashes,
        "artifacts": {
            name: {"bytes": path.stat().st_size}
            for name, path in inputs.items()
            if name in {*outputs, "baseline", "inventory_baseline", "rust_bigint"}
        },
        "rust_build_record": reference,
        "relar_build_provenance": prior.build_provenance(build),
        "baseline_provenance": {
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
        }
        if baseline
        else None,
        "inventory_baseline_provenance": {
            key: inventory_baseline[key]
            for key in (
                "git",
                "sources",
                "sha256",
                "tools",
                "options",
                "compilation",
                "relar_build_provenance",
            )
        }
        if inventory_baseline
        else None,
        "inventory_baseline_source": inventory_baseline_source,
        "tools": tools,
        "execution": execution,
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
            "target": target,
            "requested_target": args.target,
            "lto": "thin" if thin else None,
            "llvm_budget": 1000 if thin else None,
            "samples": args.samples,
            "warmups": args.warmups,
            "timeout_seconds": args.timeout,
            "validation_only": args.validate_only,
        },
        "method": {
            "timer": "time.perf_counter_ns around subprocess.run; fresh process for every engine/sample",
            "includes": "startup/imports, file opens/reads, strict decoding, parsing, eager comprehension construction, fulfillment, full rendering/writes/flush/close, and process exit",
            "excludes": "builds, oracles, fixture creation, warmups, output/filesystem comparison and hashing, fixture deletion",
            "order_cycles": {
                name: tuple(itertools.permutations(group))
                for name, group in commands.items()
            },
            "ordering": "each complete engine permutation cycle repeats evenly; positions and directed within-round adjacent pairs balanced; warmups and cross-round predecessors not balanced; no discarded samples",
            "filesystem": "fresh isolated cwd per engine/run; complete input bytes captured once per workload; inputs created before timing and OS caches not deliberately flushed; buffered output with explicit flush/close, no fsync or atomic replacement",
            "output": "complete stdout/stderr/status and full directory bytes compared for every gate/warmup/sample; expected changed-file bytes once per workload, per-run complete file hashes/lengths",
            "hash_seeds": "default per-process hash seeding; report order never depends on set traversal",
            "compile_commands": compile_commands,
            "commands": commands,
            "scope": "new whole-program inventory comparison; optional unchanged incremental controls quantify compiler/runtime changes independently; Rust uses the same eager algorithm with BigInt and native containers, not identical allocation representation; no C/general-language-speed claim",
        },
        "validation": validation,
        "workloads": workloads,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    for option in (
        "compiler",
        "runtime",
        "relar-build-record",
        "rust-bigint",
        "rust-build-record",
    ):
        parser.add_argument("--" + option, type=Path, required=True)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--baseline-record", type=Path)
    parser.add_argument("--inventory-baseline", type=Path)
    parser.add_argument("--inventory-baseline-record", type=Path)
    parser.add_argument(
        "--inventory-baseline-source-root",
        type=Path,
        help="explicitly compare changed application source against a verified old inventory_reconciliation package snapshot",
    )
    parser.add_argument("--rust-source-root", type=Path, default=REFERENCE)
    parser.add_argument("--python", default="python3.14")
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--target")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument(
        "--artifact-dir", type=Path, default=HERE / "local/inventory-planner"
    )
    parser.add_argument(
        "--output", type=Path, default=HERE / "local/inventory-comparison.json"
    )
    parser.add_argument("--workload", action="append", dest="workloads")
    parser.add_argument("--control-workload", action="append", dest="control_workloads")
    parser.add_argument("--samples", type=shared.positive_int)
    parser.add_argument("--warmups", type=shared.positive_int, default=2)
    parser.add_argument("--cpu", type=int)
    parser.add_argument("--timeout", type=shared.positive_float, default=120)
    parser.add_argument("--validate-only", action="store_true")
    args = parser.parse_args()
    if bool(args.baseline) != bool(args.baseline_record):
        parser.error("--baseline and --baseline-record must be supplied together")
    if args.control_workloads and not args.baseline:
        parser.error("--control-workload requires a baseline")
    if bool(args.inventory_baseline) != bool(args.inventory_baseline_record):
        parser.error(
            "--inventory-baseline and --inventory-baseline-record must be supplied together"
        )
    if args.inventory_baseline_source_root and not args.inventory_baseline:
        parser.error("--inventory-baseline-source-root requires an inventory baseline")
    engines = 4 if args.inventory_baseline else 3
    if args.samples is None:
        args.samples = 48 if args.inventory_baseline else 30
    cycle = math.factorial(engines)
    if args.samples % cycle:
        parser.error(f"--samples must be a multiple of {cycle}")
    if args.lto_linker and not args.target:
        parser.error("--lto-linker requires explicit --target")
    result = benchmark(args)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.output.with_name(args.output.name + ".tmp")
    temporary.write_text(json.dumps(result, indent=2, ensure_ascii=False) + "\n")
    temporary.replace(args.output)
    print(f"Saved {args.output}")


if __name__ == "__main__":
    main()
