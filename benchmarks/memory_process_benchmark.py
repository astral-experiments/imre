"""Prepared-project process benchmarks, with separate parse/compare/render controls."""

import gzip
import json
import runpy
import shutil
import subprocess
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
base = SimpleNamespace(**runpy.run_path(str(HERE / "memory_benchmark.py")))
process_api = base.process_api
validator, shared, targets = base.validator, base.shared, base.targets


def process_commands(prepared, paths):
    return {**base.commands_for(prepared, paths),
            "original": process_api.original_command(str(paths["python"]))}


def compute_commands_for(prepared, paths):
    commands = base.compute_commands_for(prepared, paths)
    original = commands["original"]
    commands["original"] = [original[0], "-P", "-s", *original[1:]]
    return commands


def describe(value, recipe, projects):
    expected = process_api.oracle(value)
    return {"label": value.label, "recipe": recipe, "projects": len(projects),
            "child_processes": 2 * len(projects), "arguments": list(value.arguments),
            "records_per_side": sum(len(project["old"].get("structs", [])) + len(project["old"].get("queries", []))
                                    for project in projects),
            "captured_stdout_bytes": sum(len(value.files[f"project-{index:04}/{role}.json"])
                                          for index in range(len(projects)) for role in ("old", "new")),
            "expected_markdown_sha256": validator.digest(expected["rendered"].encode()),
            "expected_markdown_bytes": len(expected["rendered"].encode())}


def prepare_real(artifact, paths, provenance, timeout):
    """Run both pinned ty binaries over hash-verified retained source trees."""
    projects, captures = [], []
    for name, index in (("inventory", 0), ("ruff-scripts", 2)):
        source = provenance["reports"][index]
        snapshot = artifact / "sources" / name
        for relative in source["producer"]["sources"]:
            destination = snapshot / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(paths[f"project:{name}:{relative}"], destination)
        config = snapshot / "ty.toml"
        config.write_text("")
        argv = ["{ty}", *source["producer"]["command"][1:], "--config-file", str(config)]
        reports = {}
        for role in ("old", "new"):
            command = [str(paths[role + "_ty"]) if item == "{ty}" else item for item in argv]
            completed = subprocess.run(command, cwd=snapshot, capture_output=True,
                                       env={**process_api.environment(), "TY_MEMORY_REPORT": "json", "TY_MAX_PARALLELISM": "1"},
                                       timeout=timeout, check=True)
            for stream in ("stdout", "stderr"):
                (artifact / f"{name}-{role}.{stream}").write_bytes(getattr(completed, stream))
            reports[role] = json.loads(completed.stdout)
            captures.append({"project": name, "role": role, "command": command, "cwd": str(snapshot),
                             "returncode": completed.returncode,
                             "stdout_bytes": len(completed.stdout), "stderr_bytes": len(completed.stderr),
                             "stdout_sha256": validator.digest(completed.stdout), "stderr_sha256": validator.digest(completed.stderr)})
        projects.append({"name": name, "cwd": str(snapshot), "argv": argv, **reports})
    value = process_api.case("real-prepared-ty", projects, paths["old_ty"], paths["new_ty"])
    (artifact / "real-projects.json").write_bytes(value.files["projects.json"])
    description = describe(value, "Pinned ty 0.0.89 and 0.0.90 checking retained inventory and Ruff Python sources", projects)
    description["captured_stdout_bytes"] = sum(capture["stdout_bytes"] for capture in captures)
    description["captured_stderr_bytes"] = sum(capture["stderr_bytes"] for capture in captures)
    return (description, value), captures


def save_case(directory, description, value, compute=False):
    path = directory / (value.label + ".json.gz")
    path.write_bytes(gzip.compress(json.dumps(validator.case_record(value)).encode(), mtime=0))
    return {"description": description, "case": str(path), "sha256": shared.sha256(path), "compute": compute}


def load_case(workload):
    path = Path(workload["case"])
    assert shared.sha256(path) == workload["sha256"]
    return validator.case_from_record(json.loads(gzip.decompress(path.read_bytes())))


def prepare_workloads(args, prepared, paths, provenance):
    directory = args.artifact_dir.resolve() / "process-inputs"
    directory.mkdir()
    for role in ("old", "new"):
        shutil.copyfile(paths["producer"], directory / ("capture-" + role))
        (directory / ("capture-" + role)).chmod(0o755)
    real, captures = prepare_real(directory, paths, provenance, args.timeout)
    workloads = [save_case(directory, *real)]
    empty = [{"name": "empty", "old": validator.report(), "new": validator.report()}]
    control = process_api.case("process-empty-control", empty, directory / "capture-old", directory / "capture-new")
    workloads.append(save_case(directory, describe(control, "Two native child launches with empty schema-valid reports", empty), control))
    old = json.loads((validator.PACKAGE / "sample/ruff-scripts-old.json").read_text())
    new = json.loads((validator.PACKAGE / "sample/ruff-scripts.json").read_text())
    for count, copies in args.process_layouts:
        before, after = base.scaled(old, copies), base.scaled(new, copies)
        projects = [{"name": f"project-{index:04}", "old": before, "new": after,
                     "stderr": b"captured ty warning\r\n" * (1024 * copies)} for index in range(count)]
        value = process_api.case(f"captured-process-{count}x{copies}", projects,
                                 directory / "capture-old", directory / "capture-new")
        description = describe(value, "Shared native child emits distinct-name copies of retained Ruff reports and captured stderr", projects)
        description["copies_per_project"] = copies
        description["captured_stderr_bytes"] = sum(2 * len(project["stderr"]) for project in projects)
        workloads.append(save_case(directory, description, value))
        names = [project["name"] + ".json" for project in projects]
        files = validator.directory_case(f"process-compute-{count}x{copies}", dict.fromkeys(names, before), dict.fromkeys(names, after))
        for rounds in (0, args.compute_rounds):
            compute = validator.compute_case(files, rounds)
            expected = validator.compute_oracle(compute)
            details = {**description, "label": compute.label, "child_processes": 0, "rounds": rounds,
                       "arguments": list(compute.arguments), "recipe": "Preloaded same reports; repeated decode/compare/render without children",
                       "expected_markdown_sha256": validator.digest(expected["rendered"].encode()),
                       "expected_markdown_bytes": len(expected["rendered"].encode())}
            workloads.append(save_case(directory, details, compute, compute=True))
    commands = process_commands(prepared, paths)
    compute_commands = compute_commands_for(prepared, paths)
    gates = {}
    for workload in workloads:
        value = load_case(workload)
        is_compute = workload["compute"]
        expected = validator.compute_oracle(value) if is_compute else process_api.oracle(value)
        check = validator.check_compute_result if is_compute else process_api.check_result
        gates[value.label] = {}
        for engine, command in (compute_commands if is_compute else commands).items():
            with tempfile.TemporaryDirectory(prefix="memory-process-gate-") as temporary:
                cwd = Path(temporary)
                validator.materialize(value, cwd)
                completed = subprocess.run([*command, *value.arguments], input=b"", capture_output=True,
                                           cwd=cwd, env=process_api.environment(), timeout=args.timeout, check=False)
                gates[value.label][engine] = check(value, expected, completed, cwd, engine)
    (directory / "workload-validation.json").write_text(json.dumps(gates, indent=2) + "\n")
    return {"workloads": workloads, "captures": captures, "directory": str(directory),
            "files": {str(path): shared.sha256(path) for path in sorted(directory.rglob("*")) if path.is_file()},
            "directories": [str(path) for path in sorted(directory.rglob("*")) if path.is_dir()],
            "environment": process_api.environment(), "process_layouts": args.process_layouts,
            "compute_rounds": args.compute_rounds, "input_guards_passed": True}


def verify_processes(record, args):
    assert record["input_guards_passed"] and record["process_layouts"] == args.process_layouts
    assert record["compute_rounds"] == args.compute_rounds
    assert record["environment"] == process_api.environment()
    assert record["files"] == {str(path): shared.sha256(path) for path in sorted(Path(record["directory"]).rglob("*")) if path.is_file()}
    assert record["directories"] == [str(path) for path in sorted(Path(record["directory"]).rglob("*")) if path.is_dir()]


def main():
    parser = base.argument_parser()
    parser.description = __doc__
    parser.add_argument("--old-ty", required=True)
    parser.add_argument("--new-ty", required=True)
    parser.add_argument("--ruff-source", type=Path, default=base.ROOT.parent / "ruff")
    parser.add_argument("--process-layouts", nargs="+", default=["64:1", "2:32"])
    args = parser.parse_args()
    args.process_layouts = [list(map(shared.positive_int, item.split(":"))) for item in args.process_layouts]
    assert all(len(item) == 2 for item in args.process_layouts)
    assert len({tuple(item) for item in args.process_layouts}) == len(args.process_layouts)
    assert args.samples % 6 == 0, "samples must balance all six three-engine permutations"
    assert not (args.prepare_only and args.prepared_record)
    paths, rust_record = base.source_paths(args)
    provenance = json.loads((validator.PACKAGE / "sample/provenance.json").read_text())
    for role, index in (("old", 0), ("new", 1)):
        paths[role + "_ty"] = Path(shared.executable(getattr(args, role + "_ty")))
        assert shared.sha256(paths[role + "_ty"]) == provenance["reports"][index]["producer"]["binary_sha256"]
    for name, index, source in (("inventory", 0, base.ROOT), ("ruff-scripts", 2, args.ruff_source)):
        for relative, digest in provenance["reports"][index]["producer"]["sources"].items():
            paths[f"project:{name}:{relative}"] = (source / relative).resolve(strict=True)
            assert shared.sha256(paths[f"project:{name}:{relative}"]) == digest
    hashes = {name: shared.sha256(path) for name, path in paths.items()}
    revision = targets.run_checked(["git", "-C", str(base.ROOT), "rev-parse", "HEAD"]).strip()
    dirty = bool(targets.run_checked(["git", "-C", str(base.ROOT), "status", "--porcelain"]).strip())
    assert args.allow_dirty or not dirty, "commit measured sources first, or label exploration with --allow-dirty"
    artifact = args.artifact_dir.resolve()
    assert not artifact.exists() or not any(artifact.iterdir()), "use a fresh artifact directory"
    protected = [*paths.values(), *([args.prepared_record.resolve(strict=True)] if args.prepared_record else [])]
    assert all(not artifact.is_relative_to(path) and not path.is_relative_to(artifact) for path in protected)
    assert all(args.output.resolve() != path.resolve() and not (args.output.exists() and args.output.samefile(path)) for path in protected)
    assert not args.output.resolve().is_relative_to(artifact)
    artifact.mkdir(parents=True, exist_ok=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    preparation_hash = shared.sha256(args.prepared_record) if args.prepared_record else None
    if args.prepared_record:
        prepared = json.loads(args.prepared_record.read_text())
    else:
        prepared = base.prepare(args, paths, hashes, rust_record, revision, dirty)
        prepared["processes"] = prepare_workloads(args, prepared, paths, provenance)
    base.verify(prepared, paths, hashes, args, revision, dirty)
    verify_processes(prepared["processes"], args)
    retained = [Path(prepared[name]) for name in ("binary", "llvm", "validation", "compute_binary", "compute_llvm", "compute_validation", "process_validation")]
    retained.extend(map(Path, prepared["processes"]["files"]))
    assert all(args.output.resolve() != path.resolve() and not (args.output.exists() and args.output.samefile(path)) for path in retained)
    if args.prepared_record:
        assert all(not artifact.is_relative_to(path) and not path.is_relative_to(artifact) for path in retained)
    if args.prepare_only:
        record = prepared
    else:
        cpu = shared.pin_cpu(args.cpu)
        commands = {name: command for name, command in process_commands(prepared, paths).items() if name != "port"}
        compute_commands = {name: command for name, command in compute_commands_for(prepared, paths).items() if name != "port"}
        python_info = json.loads(subprocess.run([str(paths["python"]), "-B", "-P", "-s", "-c", shared.PYTHON_INFO],
                                              env=process_api.environment(), capture_output=True, text=True, check=True).stdout)
        assert python_info["implementation"] == "cpython" and python_info["jit_enabled"] in (False, None)
        record = {"schema": 1, "kind": "memory-process-benchmark", "started_at_utc": datetime.now(timezone.utc).isoformat(),
                  "revision": revision, "dirty": dirty, "preparation": prepared, "preparation_record_sha256": preparation_hash,
                  "cpu": cpu, "python": python_info,
                  "samples": args.samples, "warmups": args.warmups, "workloads": [], "compute_workloads": [],
                  "method": "Whole-process startup, manifest decode, two sequential children per project, complete UTF-8 stdout/stderr capture, report decoding, comparison, ranking, rendering, and one output write. The real workload includes complete pinned ty checking; synthetic workloads use identical native producer bytes for every engine. All six engine permutations balance positions and directed within-round neighbors. Validation and file hashing occur outside timing. Inputs are page-cache warm; output is not fsynced.",
                  "baseline": "CPython prepared-project adapter using unchanged upstream dataclasses and rendering; not the upstream setup/download CLI. The typed CPython port is also validated on every workload but excluded from timings.",
                  "compute_method": "Separate preloaded-text entrypoints repeat the same decode/compare/render work without child launches. Whole-process time divided by repetitions is an amortized estimate including preload/startup/final output. Zero-repetition preload controls are reported without subtraction. Rust black-boxes inputs and output checksum contributions. Complete last output and Unicode-length checksum are checked outside timing.",
                  "input_guards_passed": False}
        for workload in prepared["processes"]["workloads"]:
            value = load_case(workload)
            computed = workload["compute"]
            result = base.measure(workload["description"], value, compute_commands if computed else commands,
                                  artifact / value.label, args, compute=computed, prepared_process=not computed,
                                  environment_override=process_api.environment())
            if computed and result["rounds"]:
                result["amortized_seconds"] = {name: item["median_seconds"] / result["rounds"] for name, item in result["summary"].items()}
            record["compute_workloads" if computed else "workloads"].append(result)
            args.output.write_text(json.dumps(record, indent=2) + "\n")
            print(f"{value.label}: original/Relar={result['original_over_relar']:.3f}, Relar/Rust={result['relar_over_rust']:.3f}", flush=True)
    final_hashes = {name: shared.sha256(path) for name, path in paths.items()}
    assert hashes == final_hashes, {name: (digest, final_hashes.get(name)) for name, digest in hashes.items() if final_hashes.get(name) != digest}
    base.verify(prepared, paths, hashes, args, revision, dirty)
    verify_processes(prepared["processes"], args)
    if args.prepared_record:
        assert preparation_hash == shared.sha256(args.prepared_record)
    assert revision == targets.run_checked(["git", "-C", str(base.ROOT), "rev-parse", "HEAD"]).strip()
    assert dirty == bool(targets.run_checked(["git", "-C", str(base.ROOT), "status", "--porcelain"]).strip())
    record["input_guards_passed"] = True
    record["completed_at_utc"] = datetime.now(timezone.utc).isoformat()
    args.output.write_text(json.dumps(record, indent=2) + "\n")
    print(f"Saved {args.output}")


if __name__ == "__main__":
    main()
