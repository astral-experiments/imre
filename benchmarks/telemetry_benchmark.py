"""Compare complete telemetry queries on Relar, CPython, and independent Rust."""

import argparse
import itertools
import json
import platform
import random
import runpy
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
REFERENCE = HERE / "reference/telemetry_comparison"
shared = SimpleNamespace(**runpy.run_path(str(HERE / "benchmark.py")))
validator = SimpleNamespace(**runpy.run_path(str(HERE / "telemetry_validate.py")))
SCENARIOS = (
    "report-mixed",
    "alert-quiet",
    "alert-early",
    "within-quiet",
    "within-early",
    "latest-quiet",
    "latest-recent",
)


def run_checked(command: list[str], timeout: float = 600) -> str:
    result = subprocess.run(
        command, input=b"", capture_output=True, timeout=timeout, check=False
    )
    if result.returncode or result.stderr:
        raise RuntimeError((command, result.returncode, result.stdout, result.stderr))
    return result.stdout.decode()


def rust_host(record: dict) -> str:
    return next(
        line.removeprefix("host: ")
        for line in record["rustc_verbose"].splitlines()
        if line.startswith("host: ")
    )


def elf_target(path: Path) -> dict:
    with path.open("rb") as stream:
        header = stream.read(20)
    assert len(header) == 20 and header[:5] == b"\x7fELF\x02", "expected 64-bit ELF"
    byte_order = {1: "little", 2: "big"}[header[5]]
    return {
        "bits": 64,
        "byte_order": byte_order,
        "machine": int.from_bytes(header[18:20], byte_order),
    }


def workloads(sizes: list[int], batch_pairs: int, scenarios) -> list[dict]:
    generator = random.Random(20261020)
    result = []
    for size in sizes:
        reference = [
            (1000 + position * 10, generator.randrange(-10000, 10001))
            for position in range(size)
        ]
        for scenario in scenarios:
            operation, pattern = scenario.split("-")
            errors = [position % 7 - 3 for position in range(size)]
            if pattern == "mixed":
                errors = [(position * 13 + 5) % 19 - 9 for position in range(size)]
            elif pattern == "early":
                errors[0] = 9
            elif pattern == "recent":
                errors[-1] = 9
            observed = [
                (stamp, value + error)
                for (stamp, value), error in zip(reference, errors, strict=True)
            ]
            breached = [
                position for position, error in enumerate(errors) if abs(error) > 3
            ]
            forward = breached[0] + 1 if breached else size
            backward = size - breached[-1] if breached else size
            visits = (
                3 * size + 2 * forward + backward
                if operation == "report"
                else backward
                if operation == "latest"
                else forward
            )
            repetitions = max(1, batch_pairs // visits)
            value = validator.case(
                f"{scenario}-{size}", reference, observed, operation, 3, repetitions
            )
            expected = validator.oracle(value)
            assert expected["stdout"].startswith("OK\n")
            result.append(
                {
                    "label": value.label,
                    "scenario": scenario,
                    "records": size,
                    "inspected_pairs_per_call": visits,
                    "repetitions": repetitions,
                    "case": validator.case_record(value),
                    "expected_stdout": expected["stdout"],
                    "expected_files": validator.encoded_files(expected["files"]),
                }
            )
    return result


def measure(workload: dict, commands: dict, directory: Path, args) -> dict:
    value = validator.case_from_record(workload["case"])
    expected = validator.oracle(value)
    assert expected["stdout"] == workload["expected_stdout"]
    assert validator.encoded_files(expected["files"]) == workload["expected_files"]
    directories = {name: directory / name for name in commands}
    for path in directories.values():
        path.mkdir(parents=True)
        validator.materialize(value, path)

    def run(name: str) -> dict:
        path = directories[name]
        output = path / "report.txt"
        output.write_bytes(b"replace this complete report")
        command = [*commands[name], *value.arguments]
        start = time.perf_counter_ns()
        process = subprocess.run(
            command,
            input=b"",
            capture_output=True,
            cwd=path,
            timeout=args.timeout,
            check=False,
        )
        elapsed = time.perf_counter_ns() - start
        checked = validator.check_result(value, expected, process, path)
        return {"command": command, "elapsed_ns": elapsed, **checked}

    gates = {name: run(name) for name in commands}
    orders = tuple(itertools.permutations(commands))
    phases = {}
    for phase, count in (("warmups", args.warmups), ("samples", args.samples)):
        rounds = []
        for index in range(count):
            order = orders[index % len(orders)]
            sample = {"index": index, "order": order}
            for name in order:
                sample[name] = run(name)
            rounds.append(sample)
        phases[phase] = rounds
    summaries = {name: shared.summary(phases["samples"], name) for name in commands}
    ratios = {
        f"{left}_over_{right}": summaries[left]["median_seconds"]
        / summaries[right]["median_seconds"]
        for left, right in (("cpython", "relar"), ("relar", "rust"), ("cpython", "rust"))
    }
    return {**workload, "gates": gates, **phases, "summary": summaries, **ratios}


def source_paths(args) -> tuple[dict[str, Path], dict, dict | None]:
    rust = Path(shared.executable(str(args.rust)))
    record_path = (args.rust_build_record or rust.parent / "build-record.json").resolve(
        strict=True
    )
    rust_record = json.loads(record_path.read_text())
    paths = {
        "compiler": Path(shared.executable(args.compiler)),
        "runtime": args.runtime.resolve(strict=True),
        "python": Path(shared.executable(args.python)),
        "clang": Path(shared.executable(args.clang)),
        "rust": rust,
        "rust_build_record": record_path,
        "harness": Path(__file__).resolve(),
        "validator": HERE / "telemetry_validate.py",
        "shared_harness": HERE / "benchmark.py",
        "shared_validator": HERE / "validate.py",
        "filesystem_validator": HERE / "manifest_validate.py",
        "batch_validator": HERE / "batch_validate.py",
        "incremental_validator": HERE / "incremental_validate.py",
    }
    paths.update(
        {
            "example:" + str(path.relative_to(validator.PACKAGE)): path
            for path in sorted(validator.PACKAGE.rglob("*"))
            if path.is_file() and "__pycache__" not in path.parts
        }
    )
    rust_sources = {
        str(path.relative_to(REFERENCE)): path
        for path in (
            REFERENCE / "Cargo.toml",
            REFERENCE / "Cargo.lock",
            REFERENCE / "build.py",
            *sorted((REFERENCE / "src").glob("*.rs")),
        )
    }
    paths.update({"rust_source:" + name: path for name, path in rust_sources.items()})
    assert rust_record["sources"] == {
        name: shared.sha256(path) for name, path in rust_sources.items()
    }
    assert all(
        validator.digest(rust_record["source_contents"][name].encode()) == value
        for name, value in rust_record["sources"].items()
    )
    assert (
        rust_record["entrypoint"] == "telemetry" and rust_record["input_guards_passed"]
    )
    assert rust_record["binary_sha256"] == shared.sha256(rust)
    assert rust_record["binaries"]["reference"]["sha256"] == shared.sha256(rust)
    assert rust_record["native_artifact"]["fresh"] is False
    rust_target = rust_host(rust_record)
    assert args.target is None or args.target == rust_target, (
        "Rust and Relar targets must match"
    )
    thinlto = args.lto_linker is not None
    assert rust_record["thinlto"] == thinlto
    environment = rust_record["environment"]
    assert environment["CARGO_PROFILE_RELEASE_OPT_LEVEL"] == "3"
    assert environment["CARGO_PROFILE_RELEASE_LTO"] == ("thin" if thinlto else "off")
    assert environment["CARGO_PROFILE_RELEASE_CODEGEN_UNITS"] == "16"
    assert rust_record["llvm_budget"] == (1000 if thinlto else None)
    assert environment["RUSTFLAGS"] == (
        "-Cllvm-args=-import-instr-limit=1000 -Cllvm-args=-inline-threshold=1000"
        if thinlto
        else ""
    )
    assert environment["CARGO_ENCODED_RUSTFLAGS"] == (None if thinlto else "")
    if args.lto_linker:
        paths["lto_linker"] = args.lto_linker.absolute()
    relar_record = None
    if args.relar_build_record:
        paths["relar_build_record"] = args.relar_build_record.resolve(strict=True)
        relar_record = json.loads(paths["relar_build_record"].read_text())
        runtime_key = "thinlto_runtime" if thinlto else "normal_runtime"
        assert shared.sha256(paths["compiler"]) == relar_record["sha256"]["compiler"]
        assert shared.sha256(paths["runtime"]) == relar_record["sha256"][runtime_key]
        for flag in ("source_matches_revision", "input_guards_passed"):
            assert relar_record.get(flag, True) is True, flag
        assert relar_record["target"] == rust_target
        for name, value in relar_record["source_sha256"].items():
            paths["relar_source:" + name] = ROOT / name
            assert shared.sha256(ROOT / name) == value
    return paths, rust_record, relar_record


def prepare(args, paths, hashes, rust_record, relar_record, revision, dirty) -> dict:
    artifacts = args.artifact_dir.resolve()
    binary, llvm, validation_path = (
        artifacts / "telemetry",
        artifacts / "telemetry.ll",
        artifacts / "validation.json",
    )
    command = [
        str(paths["compiler"]),
        "build",
        str(validator.ENTRY),
        "--module-root",
        str(validator.PACKAGE_ROOT),
        "-o",
        str(binary),
        "--runtime",
        str(paths["runtime"]),
        "--clang",
        str(paths["clang"]),
        "--opt-level",
        "3",
        "--emit-llvm",
        str(llvm),
    ]
    effective_target = args.target or rust_host(rust_record)
    command += ["--target", effective_target]
    if args.lto_linker:
        command += ["--lto-linker", str(paths["lto_linker"])]
    artifacts.mkdir(parents=True, exist_ok=True)
    start = time.perf_counter_ns()
    run_checked(command)
    elapsed = time.perf_counter_ns() - start
    validation_command = [
        str(paths["python"]),
        "-I",
        "-B",
        str(HERE / "telemetry_validate.py"),
        "--binary",
        str(binary),
        "--rust",
        str(paths["rust"]),
        "--output",
        str(validation_path),
    ]
    run_checked(validation_command)
    validation = json.loads(validation_path.read_text())
    assert validation["input_guards_passed"]
    assert validation["source_and_binary_sha256"]["relar"] == shared.sha256(binary)
    assert validation["source_and_binary_sha256"]["rust"] == hashes["rust"]
    native_elf = elf_target(binary)
    assert native_elf == elf_target(paths["rust"]), "Relar and Rust ELF targets differ"
    assert hashes == {name: shared.sha256(path) for name, path in paths.items()}
    return {
        "schema": 1,
        "kind": "telemetry-preparation",
        "completed_at_utc": datetime.now(timezone.utc).isoformat(),
        "revision": revision,
        "dirty": dirty,
        "compile_command": command,
        "build_ns": elapsed,
        "binary": str(binary),
        "binary_sha256": shared.sha256(binary),
        "llvm": str(llvm),
        "llvm_sha256": shared.sha256(llvm),
        "effective_target": effective_target,
        "native_elf": native_elf,
        "validation_path": str(validation_path),
        "validation_sha256": shared.sha256(validation_path),
        "validation_command": validation_command,
        "validation": validation,
        "requested_target": args.target,
        "thinlto": args.lto_linker is not None,
        "source_and_tool_paths": {name: str(path) for name, path in paths.items()},
        "source_and_tool_sha256": hashes,
        "relar_build_record": relar_record,
        "rust_build_record": rust_record,
        "input_guards_passed": True,
    }


def verify_preparation(record: dict, args, paths, hashes, revision, dirty) -> None:
    assert record["kind"] == "telemetry-preparation" and record["input_guards_passed"]
    assert record["revision"] == revision and record["dirty"] == dirty
    assert record["requested_target"] == args.target and record["thinlto"] == (
        args.lto_linker is not None
    )
    assert record["effective_target"] == (
        args.target or rust_host(record["rust_build_record"])
    )
    command = record["compile_command"]
    assert command[command.index("--target") + 1] == record["effective_target"]
    assert (
        record["native_elf"]
        == elf_target(Path(record["binary"]))
        == elf_target(paths["rust"])
    )
    assert record["source_and_tool_paths"] == {
        name: str(path) for name, path in paths.items()
    }
    assert record["source_and_tool_sha256"] == hashes
    for name in ("binary", "llvm", "validation"):
        path = Path(record[name if name != "validation" else "validation_path"])
        assert record[name + "_sha256"] == shared.sha256(path)
    assert record["validation"] == json.loads(
        Path(record["validation_path"]).read_text()
    )
    assert record["validation"]["input_guards_passed"]
    assert (
        record["validation"]["source_and_binary_sha256"]["relar"]
        == record["binary_sha256"]
    )
    assert record["validation"]["source_and_binary_sha256"]["rust"] == hashes["rust"]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compiler", required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--relar-build-record", type=Path)
    parser.add_argument("--python", default="python3")
    parser.add_argument("--rust", type=Path, required=True)
    parser.add_argument("--rust-build-record", type=Path)
    parser.add_argument("--clang", default="clang-18")
    parser.add_argument("--target")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--artifact-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--prepare-only", action="store_true")
    parser.add_argument("--prepared-record", type=Path)
    parser.add_argument("--samples", type=shared.positive_int, default=12)
    parser.add_argument("--warmups", type=shared.positive_int, default=8)
    parser.add_argument("--batch-pairs", type=shared.positive_int, default=1024 * 1024)
    parser.add_argument(
        "--sizes", nargs="+", type=shared.positive_int, default=[16, 256, 4096]
    )
    parser.add_argument("--scenarios", nargs="+", choices=SCENARIOS, default=SCENARIOS)
    parser.add_argument("--timeout", type=shared.positive_float, default=120)
    parser.add_argument("--cpu", type=int)
    parser.add_argument("--allow-dirty", action="store_true")
    args = parser.parse_args()
    if args.prepare_only and args.prepared_record:
        parser.error("--prepare-only and --prepared-record are mutually exclusive")
    if args.samples < 6 or args.samples % 6:
        parser.error("samples must be a positive multiple of the six-order schedule")
    if len(set(args.sizes)) != len(args.sizes) or len(set(args.scenarios)) != len(
        args.scenarios
    ):
        parser.error("sizes and scenarios must not repeat")
    paths, rust_record, relar_record = source_paths(args)
    artifacts = args.artifact_dir.resolve()
    selected = workloads(args.sizes, args.batch_pairs, args.scenarios)
    labels = ["empty-control", *(item["label"] for item in selected)]
    if any(args.output.resolve().is_relative_to(artifacts / label) for label in labels):
        parser.error("output cannot be inside a measurement input/output directory")
    protected = list(paths.values())
    if args.prepared_record:
        protected.append(args.prepared_record.resolve(strict=True))
    outputs = [args.output.resolve()]
    if not args.prepared_record:
        outputs += [
            artifacts / name
            for name in ("telemetry", "telemetry.ll", "validation.json")
        ]
    for output in outputs:
        assert all(
            output.resolve() != path.resolve()
            and not (output.exists() and path.exists() and output.samefile(path))
            for path in protected
        ), "output overlaps a source, tool, or other output"
        protected.append(output)
    hashes = {name: shared.sha256(path) for name, path in paths.items()}
    revision = run_checked(["git", "-C", str(ROOT), "rev-parse", "HEAD"]).strip()
    dirty = bool(run_checked(["git", "-C", str(ROOT), "status", "--porcelain"]).strip())
    if dirty and not args.allow_dirty:
        raise ValueError(
            "commit measured sources first; --allow-dirty is for exploration"
        )
    preparation_hash = (
        shared.sha256(args.prepared_record) if args.prepared_record else None
    )
    prepared = (
        json.loads(args.prepared_record.read_text())
        if args.prepared_record
        else prepare(args, paths, hashes, rust_record, relar_record, revision, dirty)
    )
    verify_preparation(prepared, args, paths, hashes, revision, dirty)
    for name in ("binary", "llvm", "validation_path"):
        protected_path = Path(prepared[name])
        assert args.output.resolve() != protected_path.resolve() and not (
            args.output.exists() and args.output.samefile(protected_path)
        ), "output overlaps a prepared artifact"
    assert (
        revision == run_checked(["git", "-C", str(ROOT), "rev-parse", "HEAD"]).strip()
    )
    assert dirty == bool(
        run_checked(["git", "-C", str(ROOT), "status", "--porcelain"]).strip()
    )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    if args.prepare_only:
        args.output.write_text(json.dumps(prepared, indent=2) + "\n")
        print(f"Prepared and validated {prepared['binary']}")
        return
    cpu = shared.pin_cpu(args.cpu)
    python_info = json.loads(
        run_checked([str(paths["python"]), "-I", "-c", shared.PYTHON_INFO])
    )
    assert python_info["implementation"] == "cpython"
    commands = {
        "relar": [prepared["binary"]],
        "cpython": validator.python_command(str(paths["python"])),
        "rust": [str(paths["rust"])],
    }
    control_case = validator.case("empty-control", [], [], "report", 3, 1)
    expected = validator.oracle(control_case)
    control = {
        "label": "empty-control",
        "scenario": "empty-control",
        "records": 0,
        "inspected_pairs_per_call": 0,
        "repetitions": 1,
        "case": validator.case_record(control_case),
        "expected_stdout": expected["stdout"],
        "expected_files": validator.encoded_files(expected["files"]),
    }
    record = {
        "schema": 1,
        "kind": "telemetry-benchmark",
        "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "revision": revision,
        "dirty": dirty,
        "platform": platform.platform(),
        "cpu": cpu,
        "python": python_info,
        "preparation": prepared,
        "preparation_record_sha256": preparation_hash,
        "compiler_version": run_checked([str(paths["compiler"]), "--version"]).strip(),
        "clang_version": run_checked([str(paths["clang"]), "--version"]).strip(),
        "source_and_tool_sha256": hashes,
        "binary_sha256": prepared["binary_sha256"],
        "thinlto": args.lto_linker is not None,
        "requested_target": args.target,
        "batch_pairs": args.batch_pairs,
        "samples": args.samples,
        "warmups": args.warmups,
        "engine_orders": tuple(itertools.permutations(commands)),
        "order_method": "Measured rounds repeat all six engine permutations, balancing positions and within-round directed predecessors. Warmup rounds follow the same schedule but eight warmups do not form a complete cycle. Cross-round transitions are not balanced.",
        "method": "Whole-process wall time, including startup, one read/parse/alignment of both inputs, repeated queries, and one complete report write. Rust black_box surrounds each repeated query and its result. Work is normalized by source-level inspected sample pairs per call; early-exit workloads repeat more calls. Empty-input startup/protocol controls are reported without subtraction. Input preparation and complete output/file checks occur outside the timing interval.",
        "comparison": "The same query passes, thresholds, repetitions, arbitrary-precision results, and errors. Python holds typed Sample objects and generator expressions; independent Rust holds contiguous structs, uses native checked i128 reductions when values fit i64, and falls back to BigInt. Rust iterator loops and Relar generated loops need not have identical allocation/layout costs.",
        "startup_control": measure(
            control, commands, artifacts / "empty-control", args
        ),
        "workloads": [],
    }
    for workload in selected:
        result = measure(workload, commands, artifacts / workload["label"], args)
        result["startup_control_fraction"] = {
            name: record["startup_control"]["summary"][name]["median_seconds"]
            / summary["median_seconds"]
            for name, summary in result["summary"].items()
        }
        record["workloads"].append(result)
        args.output.write_text(json.dumps(record, indent=2) + "\n")
        print(
            result["label"]
            + ": "
            + ", ".join(
                f"{name}={value:.3f}x"
                for name, value in result.items()
                if "_over_" in name
            ),
            flush=True,
        )
    assert hashes == {name: shared.sha256(path) for name, path in paths.items()}
    verify_preparation(prepared, args, paths, hashes, revision, dirty)
    if args.prepared_record:
        assert preparation_hash == shared.sha256(args.prepared_record)
    assert (
        revision == run_checked(["git", "-C", str(ROOT), "rev-parse", "HEAD"]).strip()
    )
    assert dirty == bool(
        run_checked(["git", "-C", str(ROOT), "status", "--porcelain"]).strip()
    )
    record["input_guards_passed"] = True
    record["completed_at_utc"] = datetime.now(timezone.utc).isoformat()
    args.output.write_text(json.dumps(record, indent=2) + "\n")


if __name__ == "__main__":
    main()
