"""Complete payout reports on Imre, CPython and independent Rust."""

import argparse
import itertools
import json
import random
import runpy
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace

if sys.flags.optimize:
    raise SystemExit("benchmark validation requires Python assertions; do not use -O")

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
REFERENCE = HERE / "reference/record_pipeline"
shared = SimpleNamespace(**runpy.run_path(str(HERE / "benchmark.py")))
targets = SimpleNamespace(**runpy.run_path(str(HERE / "telemetry_benchmark.py")))
validator = SimpleNamespace(**runpy.run_path(str(HERE / "record_validate.py")))


def workloads(sizes, batch_records):
    result = []
    generator = random.Random(20261021)
    for size in sizes:
        rows = [(index + 1, generator.randrange(-1000, 10001), generator.randrange(100))
                for index in range(size)]
        for mode in ("gross", "net"):
            for selection, limit in (("full", 99), ("selective", 9)):
                value = validator.case(f"{mode}-{selection}-{size}", rows, mode, limit,
                                       125, max(1, batch_records // size))
                result.append(describe(value, size))
    return result


def describe(value, records):
    expected = validator.oracle(value)
    assert expected["stdout"].startswith("OK\n")
    return {"label": value.label, "records": records,
            "repetitions": int(value.arguments[-1]), "case": validator.case_record(value),
            "expected_stdout": expected["stdout"],
            "expected_files": validator.encoded_files(expected["files"])}


def measure(workload, commands, directory, args):
    value = validator.case_from_record(workload["case"])
    expected = validator.oracle(value)
    directories = {name: directory / name for name in commands}
    for path in directories.values():
        path.mkdir(parents=True)
        validator.materialize(value, path)

    def run(name):
        path = directories[name]
        (path / "report.txt").write_bytes(b"replace this complete report")
        command = [*commands[name], *value.arguments]
        start = time.perf_counter_ns()
        process = subprocess.run(command, input=b"", capture_output=True, cwd=path,
                                 timeout=args.timeout, check=False)
        elapsed = time.perf_counter_ns() - start
        return {"command": command, "elapsed_ns": elapsed,
                **validator.check_result(value, expected, process, path)}

    gates = {name: run(name) for name in commands}
    orders = tuple(itertools.permutations(commands))
    phases = {}
    for phase, count in (("warmups", args.warmups), ("samples", args.samples)):
        rounds = []
        for index in range(count):
            order = orders[index % len(orders)]
            rounds.append({"index": index, "order": order, **{name: run(name) for name in order}})
        phases[phase] = rounds
    summaries = {name: shared.summary(phases["samples"], name) for name in commands}
    return {**workload, "gates": gates, **phases, "summary": summaries,
            **{f"{left}_over_{right}": summaries[left]["median_seconds"] / summaries[right]["median_seconds"]
               for left, right in (("cpython", "imre"), ("imre", "rust"), ("cpython", "rust"))}}


def source_paths(args):
    paths = {name: Path(shared.executable(getattr(args, name)))
             for name in ("compiler", "python", "clang", "rust")}
    paths["runtime"] = args.runtime.resolve(strict=True)
    paths["rust_build_record"] = (args.rust_build_record or paths["rust"].parent / "build-record.json").resolve(strict=True)
    for name in ("record_benchmark.py", "record_validate.py", "benchmark.py", "telemetry_benchmark.py",
                 "telemetry_validate.py", "validate.py", "manifest_validate.py", "batch_validate.py", "incremental_validate.py"):
        paths["helper:" + name] = HERE / name
    paths.update({"example:" + str(path.relative_to(validator.PACKAGE)): path
                  for path in validator.PACKAGE.rglob("*.py")})
    reference_paths = [REFERENCE / name for name in ("Cargo.toml", "Cargo.lock", "build.py", "src/main.rs")]
    paths.update({"rust_source:" + str(path.relative_to(REFERENCE)): path for path in reference_paths})
    rust_record = json.loads(paths["rust_build_record"].read_text())
    assert rust_record["input_guards_passed"]
    assert rust_record["binary_sha256"] == shared.sha256(paths["rust"])
    assert rust_record["sources"] == {str(path.relative_to(REFERENCE)): shared.sha256(path) for path in reference_paths}
    assert rust_record["thinlto"] == bool(args.lto_linker)
    assert rust_record["environment"]["CARGO_PROFILE_RELEASE_OPT_LEVEL"] == "3"
    assert rust_record["environment"]["CARGO_PROFILE_RELEASE_CODEGEN_UNITS"] == "16"
    assert rust_record["environment"]["CARGO_PROFILE_RELEASE_LTO"] == ("thin" if args.lto_linker else "off")
    assert rust_record["llvm_budget"] == (1000 if args.lto_linker else None)
    if args.lto_linker:
        paths["lto_linker"] = args.lto_linker.resolve(strict=True)
    if args.imre_build_record:
        paths["imre_build_record"] = args.imre_build_record.resolve(strict=True)
        build = json.loads(paths["imre_build_record"].read_text())
        assert build["sha256"]["compiler"] == shared.sha256(paths["compiler"])
        key = "thinlto_runtime" if args.lto_linker else "normal_runtime"
        assert build["sha256"][key] == shared.sha256(paths["runtime"])
        for flag in ("source_matches_revision", "input_guards_passed"):
            assert build.get(flag, True)
    return paths, rust_record


def prepare(args, paths, hashes, rust_record, revision, dirty):
    artifact = args.artifact_dir.resolve()
    binary, llvm = artifact / "record-pipeline", artifact / "record-pipeline.ll"
    target = args.target or targets.rust_host(rust_record)
    assert target == targets.rust_host(rust_record), "reference must match requested target"
    command = [str(paths["compiler"]), "build", str(validator.ENTRY), "--module-root", str(validator.PACKAGE_ROOT),
               "-o", str(binary), "--runtime", str(paths["runtime"]), "--clang", str(paths["clang"]),
               "--opt-level", "3", "--emit-llvm", str(llvm), "--target", target]
    if args.lto_linker:
        command += ["--lto-linker", str(paths["lto_linker"])]
    targets.run_checked(command)
    assert targets.elf_target(binary) == targets.elf_target(paths["rust"])
    commands = {"imre": [str(binary)], "cpython": validator.python_command(str(paths["python"])), "rust": [str(paths["rust"])]}
    validation = validator.validate_commands(commands, args.timeout)
    validation_path = artifact / "validation.json"
    validation_path.write_text(json.dumps(validation, indent=2) + "\n")
    return {"schema": 1, "kind": "record-preparation", "revision": revision, "dirty": dirty,
            "compile_command": command, "effective_target": target, "thinlto": bool(args.lto_linker),
            "source_and_tool_paths": {name: str(path) for name, path in paths.items()},
            "source_and_tool_sha256": hashes, "rust_build_record": rust_record,
            "binary": str(binary), "binary_sha256": shared.sha256(binary),
            "llvm": str(llvm), "llvm_sha256": shared.sha256(llvm),
            "validation": str(validation_path), "validation_sha256": shared.sha256(validation_path),
            "validation_cases": validation["cases"], "native_elf": targets.elf_target(binary),
            "input_guards_passed": True}


def verify(prepared, paths, hashes, args, revision, dirty):
    assert prepared["kind"] == "record-preparation" and prepared["input_guards_passed"]
    assert prepared["revision"] == revision and prepared["dirty"] == dirty
    assert prepared["source_and_tool_paths"] == {name: str(path) for name, path in paths.items()}
    assert prepared["source_and_tool_sha256"] == hashes
    assert prepared["thinlto"] == bool(args.lto_linker)
    assert prepared["effective_target"] == (args.target or targets.rust_host(prepared["rust_build_record"]))
    for name in ("binary", "llvm", "validation"):
        assert prepared[name + "_sha256"] == shared.sha256(Path(prepared[name]))
    assert prepared["native_elf"] == targets.elf_target(Path(prepared["binary"])) == targets.elf_target(paths["rust"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("compiler", "runtime", "rust", "artifact-dir", "output"):
        parser.add_argument("--" + name, required=True, type=Path if name in ("runtime", "artifact-dir", "output") else str)
    parser.add_argument("--python", default="python3")
    parser.add_argument("--clang", default="clang-18")
    parser.add_argument("--rust-build-record", type=Path)
    parser.add_argument("--imre-build-record", type=Path)
    parser.add_argument("--target")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--prepare-only", action="store_true")
    parser.add_argument("--prepared-record", type=Path)
    parser.add_argument("--allow-dirty", action="store_true")
    parser.add_argument("--samples", type=shared.positive_int, default=12)
    parser.add_argument("--warmups", type=shared.positive_int, default=2)
    parser.add_argument("--sizes", nargs="+", type=shared.positive_int, default=[32, 4096])
    parser.add_argument("--batch-records", type=shared.positive_int, default=1024 * 1024)
    parser.add_argument("--timeout", type=shared.positive_float, default=120)
    parser.add_argument("--cpu", type=int)
    args = parser.parse_args()
    assert args.samples % 6 == 0, "samples must balance all six engine orders"
    assert len(set(args.sizes)) == len(args.sizes)
    assert not (args.prepare_only and args.prepared_record)
    paths, rust_record = source_paths(args)
    hashes = {name: shared.sha256(path) for name, path in paths.items()}
    revision = targets.run_checked(["git", "-C", str(ROOT), "rev-parse", "HEAD"]).strip()
    dirty = bool(targets.run_checked(["git", "-C", str(ROOT), "status", "--porcelain"]).strip())
    assert args.allow_dirty or not dirty, "commit measured sources first, or use --allow-dirty for exploration"
    artifact = args.artifact_dir.resolve()
    assert not artifact.exists() or not any(artifact.iterdir()), "use a fresh artifact directory"
    protected = [*paths.values(), *([args.prepared_record.resolve(strict=True)] if args.prepared_record else [])]
    assert all(not artifact.is_relative_to(path) and not path.is_relative_to(artifact) for path in protected)
    assert all(args.output.resolve() != path.resolve() and not (args.output.exists() and args.output.samefile(path)) for path in protected)
    assert not args.output.resolve().is_relative_to(artifact), "keep the result JSON outside the artifact directory"
    artifact.mkdir(parents=True, exist_ok=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    preparation_hash = shared.sha256(args.prepared_record) if args.prepared_record else None
    prepared = json.loads(args.prepared_record.read_text()) if args.prepared_record else prepare(args, paths, hashes, rust_record, revision, dirty)
    verify(prepared, paths, hashes, args, revision, dirty)
    for name in ("binary", "llvm", "validation"):
        path = Path(prepared[name])
        assert args.output.resolve() != path.resolve() and not (args.output.exists() and args.output.samefile(path))
    if args.prepare_only:
        record = prepared
    else:
        cpu = shared.pin_cpu(args.cpu)
        python_info = json.loads(targets.run_checked([str(paths["python"]), "-I", "-c", shared.PYTHON_INFO]))
        assert python_info["implementation"] == "cpython"
        commands = {"imre": [prepared["binary"]], "cpython": validator.python_command(str(paths["python"])), "rust": [str(paths["rust"])]}
        control = describe(validator.case("empty-control", [], repetitions=1), 0)
        record = {"schema": 1, "kind": "record-benchmark", "started_at_utc": datetime.now(timezone.utc).isoformat(),
                  "revision": revision, "dirty": dirty, "preparation": prepared,
                  "preparation_record_sha256": preparation_hash, "cpu": cpu,
                  "python": python_info,
                  "samples": args.samples, "warmups": args.warmups, "batch_records": args.batch_records,
                  "engine_orders": list(itertools.permutations(commands)),
                  "method": "Whole-process time: startup, one input parse, repeated payout queries, checksum and one report write/close. Complete output and filesystem checks are outside timing. Six measured engine orders are balanced; cross-round transitions and warmups are not. Empty-input controls are reported without subtraction and exclude nonempty parsing costs.",
                  "comparison": "Python uses typed objects and callbacks in lazy map/filter pipelines. Rust uses contiguous i64 records, checked arithmetic and BigInt retry with retained arbitrary-precision input; iterator closures can inline. Each Rust query and result passes through black_box. Algorithms produce the same accepted payouts and full outputs; storage and dispatch costs differ.",
                  "input_guards_passed": False,
                  "startup_control": measure(control, commands, artifact / control["label"], args), "workloads": []}
        for workload in workloads(args.sizes, args.batch_records):
            measured = measure(workload, commands, artifact / workload["label"], args)
            measured["startup_control_fraction"] = {name: record["startup_control"]["summary"][name]["median_seconds"] / summary["median_seconds"]
                                                     for name, summary in measured["summary"].items()}
            record["workloads"].append(measured)
            args.output.write_text(json.dumps(record, indent=2) + "\n")
            print(f"{measured['label']}: CPython/Imre={measured['cpython_over_imre']:.3f}, Imre/Rust={measured['imre_over_rust']:.3f}", flush=True)
    assert hashes == {name: shared.sha256(path) for name, path in paths.items()}
    verify(prepared, paths, hashes, args, revision, dirty)
    if args.prepared_record:
        assert preparation_hash == shared.sha256(args.prepared_record)
    assert revision == targets.run_checked(["git", "-C", str(ROOT), "rev-parse", "HEAD"]).strip()
    assert dirty == bool(targets.run_checked(["git", "-C", str(ROOT), "status", "--porcelain"]).strip())
    record["input_guards_passed"] = True
    record["completed_at_utc"] = datetime.now(timezone.utc).isoformat()
    args.output.write_text(json.dumps(record, indent=2) + "\n")
    print(f"Saved {args.output}")


if __name__ == "__main__":
    main()
