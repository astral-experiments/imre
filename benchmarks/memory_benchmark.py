"""Compare real ty memory reports on Relar, typed CPython, upstream CPython and Rust."""

import argparse
import json
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
REFERENCE = HERE / "reference/memory_report"
shared = SimpleNamespace(**runpy.run_path(str(HERE / "benchmark.py")))
targets = SimpleNamespace(**runpy.run_path(str(HERE / "telemetry_benchmark.py")))
validator = SimpleNamespace(**runpy.run_path(str(HERE / "memory_validate.py")))


def scaled(source, copies):
    result = {key: value * copies for key, value in source.items() if type(value) is int}
    for group in ("structs", "queries"):
        result[group] = [{**item, "name": item["name"] + "::copy" + str(copy)}
                         for copy in range(copies) for item in source.get(group, [])]
    return result


def workloads(copies):
    result = []
    for name in ("inventory", "ruff-scripts"):
        old = (validator.PACKAGE / f"sample/{name}-old.json").read_bytes()
        new = (validator.PACKAGE / f"sample/{name}.json").read_bytes()
        value = validator.case("real-" + name, old, new)
        result.append((describe(value, 1, "unmodified-ty-0.0.89-to-0.0.90-" + name), value))
    old = json.loads((validator.PACKAGE / "sample/ruff-scripts-old.json").read_text())
    new = json.loads((validator.PACKAGE / "sample/ruff-scripts.json").read_text())
    for count in copies:
        before, after = scaled(old, count), scaled(new, count)
        for mode, selected in (("full", after), ("unchanged", before)):
            value = validator.case(f"scaled-{mode}-{count}", before, selected)
            recipe = "distinct-name-copies-of-ruff-scripts-" + ("old-to-new" if mode == "full" else "old-to-old")
            result.append((describe(value, count, recipe), value))
    return result


def describe(value, copies, recipe):
    expected = validator.oracle(value)
    counts = {}
    for key in ("old.json", "project.json"):
        data = json.loads(value.files[key])
        counts[key] = len(data.get("structs", [])) + len(data.get("queries", []))
    return {"label": value.label, "copies": copies, "recipe": recipe,
            "records": counts, "arguments": list(value.arguments),
            "input_files": {name: {"sha256": validator.digest(data), "bytes": len(data)}
                            for name, data in sorted(value.files.items()) if name != "report.md"},
            "expected_markdown_sha256": validator.digest(expected["rendered"].encode()),
            "expected_markdown_bytes": len(expected["rendered"].encode())}


def engine_orders(commands):
    names = tuple(commands)
    # Four balanced Williams orders: each position and each directed adjacent
    # pair occurs equally often. Cross-round transitions are not balanced.
    return tuple(tuple(names[(index + rotation) % 4] for index in (0, 1, 3, 2))
                 for rotation in range(4))


def commands_for(prepared, paths):
    return {"relar": [prepared["binary"]], "port": validator.python_command(str(paths["python"])),
            "original": validator.original_command(str(paths["python"])), "rust": [str(paths["rust"])]}


def measure(workload, value, commands, directory, args):
    expected = validator.oracle(value)
    directories = {name: directory / name for name in commands}
    for path in directories.values():
        path.mkdir(parents=True)
        validator.materialize(value, path)

    def run(name):
        path = directories[name]
        (path / "report.md").write_bytes(value.files["report.md"])
        command = [*commands[name], *value.arguments]
        start = time.perf_counter_ns()
        process = subprocess.run(command, input=b"", capture_output=True, cwd=path,
                                 env=validator.environment(), timeout=args.timeout, check=False)
        elapsed = time.perf_counter_ns() - start
        return {"command": command, "elapsed_ns": elapsed,
                **validator.check_result(value, expected, process, path, name)}

    gates = {name: run(name) for name in commands}
    orders = engine_orders(commands)
    phases = {}
    for phase, count in (("warmups", args.warmups), ("samples", args.samples)):
        rounds = []
        for index in range(count):
            order = orders[index % len(orders)]
            row = {"index": index, "order": order}
            for name in order:
                measured = run(name)
                assert {key: value for key, value in measured.items() if key != "elapsed_ns"} == {
                    key: value for key, value in gates[name].items() if key != "elapsed_ns"}
                row[name] = measured
            rounds.append(row)
        phases[phase] = rounds
    summaries = {name: shared.summary(phases["samples"], name) for name in commands}
    return {**workload, "gates": gates, **phases, "summary": summaries,
            **{f"{left}_over_{right}": summaries[left]["median_seconds"] / summaries[right]["median_seconds"]
               for left, right in (("original", "relar"), ("port", "relar"), ("relar", "rust"), ("original", "rust"), ("port", "rust"))}}


def source_paths(args):
    paths = {name: Path(shared.executable(getattr(args, name)))
             for name in ("compiler", "python", "clang", "rust")}
    paths["runtime"] = args.runtime.resolve(strict=True)
    paths["rust_build_record"] = (args.rust_build_record or paths["rust"].parent / "build-record.json").resolve(strict=True)
    for name in ("memory_benchmark.py", "memory_validate.py", "benchmark.py", "telemetry_benchmark.py",
                 "telemetry_validate.py", "validate.py", "manifest_validate.py", "batch_validate.py", "incremental_validate.py"):
        paths["helper:" + name] = HERE / name
    paths.update({"example:" + str(path.relative_to(validator.PACKAGE)): path
                  for path in validator.PACKAGE.rglob("*.py")})
    paths.update({"stdlib:" + str(path.relative_to(validator.STDLIB)): path
                  for path in validator.STDLIB.rglob("*.py")})
    paths.update({"fixture:" + path.name: path for path in (validator.PACKAGE / "sample").iterdir() if path.is_file()})
    paths.update({"upstream:" + path.name: path for path in (validator.PACKAGE / "upstream").iterdir() if path.is_file()})
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
    if args.relar_build_record:
        paths["relar_build_record"] = args.relar_build_record.resolve(strict=True)
        build = json.loads(paths["relar_build_record"].read_text())
        assert build["sha256"]["compiler"] == shared.sha256(paths["compiler"])
        key = "thinlto_runtime" if args.lto_linker else "normal_runtime"
        assert build["sha256"][key] == shared.sha256(paths["runtime"])
        for flag in ("source_matches_revision", "input_guards_passed"):
            assert build.get(flag, True)
    return paths, rust_record


def prepare(args, paths, hashes, rust_record, revision, dirty):
    artifact = args.artifact_dir.resolve()
    binary, llvm = artifact / "memory-report", artifact / "memory-report.ll"
    target = args.target or targets.rust_host(rust_record)
    assert target == targets.rust_host(rust_record), "reference must match requested target"
    command = [str(paths["compiler"]), "build", str(validator.ENTRY), "--module-root", str(validator.PACKAGE_ROOT),
               "-o", str(binary), "--runtime", str(paths["runtime"]), "--clang", str(paths["clang"]),
               "--opt-level", "3", "--emit-llvm", str(llvm), "--target", target]
    if args.lto_linker:
        command += ["--lto-linker", str(paths["lto_linker"])]
    targets.run_checked(command)
    assert targets.elf_target(binary) == targets.elf_target(paths["rust"])
    commands = commands_for({"binary": str(binary)}, paths)
    validation = validator.validate_commands(commands, args.timeout)
    validation_path = artifact / "validation.json"
    validation_path.write_text(json.dumps(validation, indent=2) + "\n")
    return {"schema": 1, "kind": "memory-preparation", "revision": revision, "dirty": dirty,
            "compile_command": command, "effective_target": target, "thinlto": bool(args.lto_linker),
            "source_and_tool_paths": {name: str(path) for name, path in paths.items()},
            "source_and_tool_sha256": hashes, "rust_build_record": rust_record,
            "binary": str(binary), "binary_sha256": shared.sha256(binary),
            "llvm": str(llvm), "llvm_sha256": shared.sha256(llvm),
            "validation": str(validation_path), "validation_sha256": shared.sha256(validation_path),
            "validation_cases": validation["cases"], "native_elf": targets.elf_target(binary),
            "input_guards_passed": True}


def verify(prepared, paths, hashes, args, revision, dirty):
    assert prepared["kind"] == "memory-preparation" and prepared["input_guards_passed"]
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
    parser.add_argument("--relar-build-record", type=Path)
    parser.add_argument("--target")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--prepare-only", action="store_true")
    parser.add_argument("--prepared-record", type=Path)
    parser.add_argument("--allow-dirty", action="store_true")
    parser.add_argument("--samples", type=shared.positive_int, default=12)
    parser.add_argument("--warmups", type=shared.positive_int, default=2)
    parser.add_argument("--copies", nargs="+", type=shared.positive_int, default=[32, 256])
    parser.add_argument("--timeout", type=shared.positive_float, default=120)
    parser.add_argument("--cpu", type=int)
    args = parser.parse_args()
    assert args.samples % 4 == 0, "samples must balance all four engine orders"
    assert len(set(args.copies)) == len(args.copies)
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
        commands = commands_for(prepared, paths)
        control_value = validator.case("empty-control", validator.report(), validator.report())
        control = describe(control_value, 0, "empty-schema-valid-reports")
        record = {"schema": 1, "kind": "memory-benchmark", "started_at_utc": datetime.now(timezone.utc).isoformat(),
                  "revision": revision, "dirty": dirty, "preparation": prepared,
                  "preparation_record_sha256": preparation_hash, "cpu": cpu,
                  "python": python_info,
                  "samples": args.samples, "warmups": args.warmups, "copies": args.copies,
                  "engine_orders": list(engine_orders(commands)), "python_environment": {"PYTHONHASHSEED": "0", "PYTHONPATH": "", "PYTHONHOME": None},
                  "method": "Whole-process time: startup, two complete file reads and typed JSON parses, comparison, stable ranking, Markdown rendering, and one report write/close. The unchanged upstream CPython script additionally echoes the report to stderr in file mode; its complete stderr is checked. Full output/filesystem validation is outside timing. Four measured Williams orders balance positions and directed within-round neighbors; cross-round transitions and warmups are not balanced. Empty-report controls are reported without subtraction and exclude nonempty parsing/comparison costs.",
                  "comparison": "Relar and the typed CPython port use concrete report/item objects and typed JSON decoding. The unchanged upstream script uses dataclasses and dynamic JSON dictionaries. Rust deserializes contiguous native i64 item records, checks arithmetic, and retries from retained input with arbitrary-precision integers on overflow; final row formatting preserves exact integer division rounding. Both ports deterministically retain first-appearance order for equal changes; upstream set-order ties are verified as equivalent and PYTHONHASHSEED=0 pins complete output. Scaled workloads are explicit distinct-name copies of captured reports; no engine repeats or skips the application in-process.",
                  "input_guards_passed": False,
                  "startup_control": measure(control, control_value, commands, artifact / control["label"], args), "workloads": []}
        for workload, value in workloads(args.copies):
            measured = measure(workload, value, commands, artifact / workload["label"], args)
            measured["startup_control_fraction"] = {name: record["startup_control"]["summary"][name]["median_seconds"] / summary["median_seconds"]
                                                     for name, summary in measured["summary"].items()}
            record["workloads"].append(measured)
            args.output.write_text(json.dumps(record, indent=2) + "\n")
            print(f"{measured['label']}: original/Relar={measured['original_over_relar']:.3f}, typed/Relar={measured['port_over_relar']:.3f}, Relar/Rust={measured['relar_over_rust']:.3f}", flush=True)
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
