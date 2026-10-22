"""Prepared-project process benchmarks, with separate parse/compare/render controls."""

import gzip
import json
import runpy
import shutil
import subprocess
import tempfile
from dataclasses import replace
from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
base = SimpleNamespace(**runpy.run_path(str(HERE / "memory_benchmark.py")))
process_api = base.process_api
validator, shared, targets = base.validator, base.shared, base.targets

TEE_SOURCE = """
import json, os, subprocess, sys
from pathlib import Path
role = Path(sys.argv[0]).name.rsplit('-', 1)[-1]
assert role in ('old', 'new')
assert os.environ['TY_MEMORY_REPORT'] == 'json'
assert os.environ['TY_MAX_PARALLELISM'] == '1'
assert os.environ['RELAR_PROCESS_SENTINEL'] == 'inherited'
command = [sys.argv[1 if role == 'old' else 2], *sys.argv[4:]]
completed = subprocess.run(command, input=b'', capture_output=True, check=False)
directory = Path(sys.argv[3]) / role
directory.mkdir(parents=True, exist_ok=False)
(directory / 'stdout.bin').write_bytes(completed.stdout)
(directory / 'stderr.bin').write_bytes(completed.stderr)
(directory / 'command.json').write_text(json.dumps({
    'command': command, 'cwd': os.getcwd(), 'returncode': completed.returncode,
}, sort_keys=True), encoding='utf-8')
sys.stdout.buffer.write(completed.stdout)
sys.stdout.buffer.flush()
sys.stderr.buffer.write(completed.stderr)
sys.stderr.buffer.flush()
sys.exit(completed.returncode)
"""


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
    persisted = {name: data for name, data in expected["files"].items() if name not in value.files}
    return {"label": value.label, "recipe": recipe, "projects": len(projects),
            "child_processes": 2 * len(projects), "arguments": list(value.arguments),
            "records_per_side": sum(len(project["old"].get("structs", [])) + len(project["old"].get("queries", []))
                                    for project in projects),
            "captured_stdout_bytes": sum(len(value.files[f"project-{index:04}/{role}.json"])
                                          for index in range(len(projects)) for role in ("old", "new")),
            "captured_stderr_bytes": sum(len(value.files.get(f"project-{index:04}/{role}.stderr.bin",
                                                             value.files[f"project-{index:04}/stderr.bin"]))
                                          for index in range(len(projects)) for role in ("old", "new")),
            "persists_transcripts": "--capture-dir" in value.arguments,
            "persisted_capture_files": len(persisted), "persisted_capture_bytes": sum(map(len, persisted.values())),
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


def real_capture_oracle(value, directory, identities):
    """Derive exact transcripts and Markdown from independently retained child bytes."""
    manifest = json.loads(value.files["projects.json"])
    observed_inputs = dict(value.files)
    raw_files, raw_directories, children = {}, {"tee-raw"}, []
    for index, project in enumerate(manifest["projects"]):
        for role in ("old", "new"):
            prefix = f"tee-raw/project-{index}/{role}"
            raw_directories.update((f"tee-raw/project-{index}", prefix))
            for filename in ("stdout.bin", "stderr.bin", "command.json"):
                relative = prefix + "/" + filename
                raw_files[relative] = (directory / relative).read_bytes()
            child = json.loads(raw_files[prefix + "/command.json"])
            expected_command = [identities[role]["path"], *project["argv"][4:]]
            assert project["argv"][1 if role == "old" else 2] == identities[role]["path"]
            assert child == {"command": expected_command, "cwd": project["cwd"], "returncode": 0}
            observed_inputs[f"project-{index:04}/{role}.json"] = raw_files[prefix + "/stdout.bin"]
            observed_inputs[f"project-{index:04}/{role}.stderr.bin"] = raw_files[prefix + "/stderr.bin"]
            children.append({"project": project["name"], "role": role, **child, "binary": identities[role],
                             "stdout_sha256": validator.digest(raw_files[prefix + "/stdout.bin"]),
                             "stderr_sha256": validator.digest(raw_files[prefix + "/stderr.bin"])})
    expected = process_api.oracle(replace(value, files=observed_inputs))
    # The invocation's report streams are witnesses, not replacements for the
    # original fixture inputs that remained untouched in the case directory.
    for name in observed_inputs:
        if name in value.files:
            expected["files"][name] = value.files[name]
        else:
            del expected["files"][name]
    expected["files"].update(raw_files)
    expected["directories"] = sorted(set(expected["directories"]) | raw_directories)
    return expected, children


def validate_real_captures(directory, value, commands, paths, timeout):
    """Check actual ty through a tee witness; this instrumentation is never timed."""
    root = directory / "real-capture-validation"
    root.mkdir()
    python = str(paths["python"])
    assert " " not in python and "\n" not in python and len(python) < 110
    wrapper_source = "#!" + python + " -I\n" + TEE_SOURCE
    wrappers = {}
    for role in ("old", "new"):
        wrapper = root / ("ty-capture-" + role)
        wrapper.write_text(wrapper_source)
        wrapper.chmod(0o755)
        wrappers[role] = {"path": str(wrapper), "sha256": shared.sha256(wrapper)}
    identities = {role: {"path": str(paths[role + "_ty"]), "sha256": shared.sha256(paths[role + "_ty"])}
                  for role in ("old", "new")}
    results = {}
    for engine, command in commands.items():
        cwd = root / engine
        cwd.mkdir()
        manifest = json.loads(value.files["projects.json"])
        for index, project in enumerate(manifest["projects"]):
            assert project["argv"][0] == "{ty}"
            project["argv"] = ["{ty}", identities["old"]["path"], identities["new"]["path"],
                               str(cwd / "tee-raw" / ("project-" + str(index))), *project["argv"][1:]]
        arguments = list(value.arguments)
        for role in ("old", "new"):
            arguments[arguments.index("--" + role + "-ty") + 1] = wrappers[role]["path"]
        current = replace(value, label="real-capture-" + engine,
                          files={**value.files, "projects.json": json.dumps(manifest).encode()},
                          arguments=(*arguments, "--capture-dir", "captures"))
        validator.materialize(current, cwd)
        actual_command = [*command, *current.arguments]
        completed = subprocess.run(actual_command, input=b"", capture_output=True, cwd=cwd,
                                   env=process_api.environment(), timeout=timeout, check=False)
        (root / (engine + ".stdout")).write_bytes(completed.stdout)
        (root / (engine + ".stderr")).write_bytes(completed.stderr)
        assert completed.returncode == 0, (engine, completed.stderr.decode(errors="replace"))
        expected, children = real_capture_oracle(current, cwd, identities)
        result = process_api.check_result(current, expected, completed, cwd, engine)
        case_path = root / (engine + ".json.gz")
        case_path.write_bytes(gzip.compress(json.dumps(validator.case_record(current)).encode(), mtime=0))
        results[engine] = {"case": str(case_path), "case_sha256": shared.sha256(case_path),
                           "directory": str(cwd), "command": actual_command, "children": children,
                           "expected_markdown_sha256": validator.digest(expected["rendered"].encode()), "result": result}
    record = {"method": "Correctness only: wrappers retain each actual pinned ty invocation's raw streams before forwarding them. Transcripts must equal strict UTF-8/universal-newline decoding of those same bytes; an independent oracle renders the observed reports. Raw ty output may vary between invocations.",
              "timed": False, "wrapper_source": wrapper_source, "wrappers": wrappers,
              "python": {"path": python, "sha256": shared.sha256(paths["python"])},
              "ty": identities, "engines": results}
    path = root / "validation.json"
    path.write_text(json.dumps(record, indent=2) + "\n")
    return {"record": str(path), "sha256": shared.sha256(path), "engine_executions": len(results)}


def save_case(directory, description, value, compute=False, filesystem=False):
    path = directory / (value.label + ".json.gz")
    path.write_bytes(gzip.compress(json.dumps(validator.case_record(value)).encode(), mtime=0))
    return {"description": description, "case": str(path), "sha256": shared.sha256(path),
            "compute": compute, "filesystem": filesystem}


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
    capture_control = replace(control, label="process-capture-empty-control",
                              arguments=(*control.arguments, "--capture-dir", "captures"))
    workloads.append(save_case(directory, describe(capture_control, "Two child launches, retained empty-report transcripts, and atomic Markdown publication", empty), capture_control))
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
        persisted = replace(value, label=f"persisted-capture-{count}x{copies}",
                            arguments=(*value.arguments, "--capture-dir", "captures"))
        persisted_description = describe(persisted, "Fixed native child output; persisted decoded transcripts and atomic Markdown publication", projects)
        persisted_description["copies_per_project"] = copies
        workloads.append(save_case(directory, persisted_description, persisted))
        names = [project["name"] + ".json" for project in projects]
        filesystem = validator.directory_case(f"process-filesystem-{count}x{copies}", dict.fromkeys(names, before), dict.fromkeys(names, after))
        filesystem_expected = validator.oracle(filesystem)
        assert validator.digest(filesystem_expected["rendered"].encode()) == description["expected_markdown_sha256"]
        filesystem_description = {**description, "label": filesystem.label, "child_processes": 0,
                                  "arguments": list(filesystem.arguments),
                                  "captured_stdout_bytes": 0, "captured_stderr_bytes": 0,
                                  "report_json_bytes": sum(len(data) for name, data in filesystem.files.items() if name.endswith(".json")),
                                  "recipe": "Same reports read from directories; decode/compare/render and atomic publication without child launches"}
        workloads.append(save_case(directory, filesystem_description, filesystem, filesystem=True))
        files = validator.directory_case(f"process-compute-{count}x{copies}", dict.fromkeys(names, before), dict.fromkeys(names, after))
        for rounds in (0, args.compute_rounds):
            compute = validator.compute_case(files, rounds)
            expected = validator.compute_oracle(compute)
            details = {**description, "label": compute.label, "child_processes": 0, "rounds": rounds,
                       "captured_stdout_bytes": 0, "captured_stderr_bytes": 0,
                       "arguments": list(compute.arguments), "recipe": "Preloaded same reports; repeated decode/compare/render without children",
                       "expected_markdown_sha256": validator.digest(expected["rendered"].encode()),
                       "expected_markdown_bytes": len(expected["rendered"].encode())}
            workloads.append(save_case(directory, details, compute, compute=True))
    commands = process_commands(prepared, paths)
    compute_commands = compute_commands_for(prepared, paths)
    filesystem_commands = base.commands_for(prepared, paths)
    gates = {}
    for workload in workloads:
        value = load_case(workload)
        is_compute = workload["compute"]
        is_filesystem = workload["filesystem"]
        expected = (validator.compute_oracle(value) if is_compute else
                    validator.oracle(value) if is_filesystem else process_api.oracle(value))
        check = (validator.check_compute_result if is_compute else
                 validator.check_result if is_filesystem else process_api.check_result)
        gates[value.label] = {}
        for engine, command in (compute_commands if is_compute else filesystem_commands if is_filesystem else commands).items():
            with tempfile.TemporaryDirectory(prefix="memory-process-gate-") as temporary:
                cwd = Path(temporary)
                validator.materialize(value, cwd)
                completed = subprocess.run([*command, *value.arguments], input=b"", capture_output=True,
                                           cwd=cwd, env=process_api.environment(), timeout=args.timeout, check=False)
                gates[value.label][engine] = check(value, expected, completed, cwd, engine)
    (directory / "workload-validation.json").write_text(json.dumps(gates, indent=2) + "\n")
    real_capture_validation = validate_real_captures(directory, real[1], commands, paths, args.timeout)
    return {"workloads": workloads, "captures": captures, "directory": str(directory),
            "real_capture_validation": real_capture_validation,
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
        filesystem_commands = {name: command for name, command in base.commands_for(prepared, paths).items() if name != "port"}
        python_info = json.loads(subprocess.run([str(paths["python"]), "-B", "-P", "-s", "-c", shared.PYTHON_INFO],
                                              env=process_api.environment(), capture_output=True, text=True, check=True).stdout)
        assert python_info["implementation"] == "cpython" and python_info["jit_enabled"] in (False, None)
        record = {"schema": 1, "kind": "memory-process-benchmark", "started_at_utc": datetime.now(timezone.utc).isoformat(),
                  "revision": revision, "dirty": dirty, "preparation": prepared, "preparation_record_sha256": preparation_hash,
                  "cpu": cpu, "python": python_info,
                  "samples": args.samples, "warmups": args.warmups, "workloads": [], "compute_workloads": [], "filesystem_workloads": [],
                  "method": "Whole-process startup, manifest decode, two sequential children per project, complete UTF-8 stdout/stderr capture, report decoding, comparison, ranking, rendering, and sibling staging/write/close/atomic replacement. Persisted-capture rows also create a fresh capture directory, publish the manifest and each decoded stream, and clean their private workspace. The real timed workload directly runs pinned ty; synthetic workloads use identical native producer bytes for every engine. All six engine permutations balance positions and directed within-round neighbors. Full validation, file hashing, and removal of the previous invocation's retained capture tree occur outside timing. Inputs are page-cache warm; output is not fsynced.",
                  "baseline": "Process rows use a CPython prepared-project adapter with unchanged upstream dataclasses/rendering and matching capture/atomic publication. Directory rows call unchanged upstream parsing/comparison/rendering through an atomic-output adapter and retain its report echo to stderr. Neither adapter is the unchanged upstream setup/download CLI. The typed CPython port is validated on every workload but excluded from timings.",
                  "filesystem_method": "Separate no-child controls scan/read the same named JSON reports, decode, compare, rank, render, and atomically publish the report. These whole-process measurements include startup and all file work. They are reported directly without subtraction from child-process rows.",
                  "compute_method": "Separate preloaded-text entrypoints repeat the same decode/compare/render work without child launches. Whole-process time divided by repetitions is an amortized estimate including preload/startup/final output. Zero-repetition preload controls are reported without subtraction. Rust black-boxes inputs and output checksum contributions. Complete last output and Unicode-length checksum are checked outside timing.",
                  "input_guards_passed": False}
        for workload in prepared["processes"]["workloads"]:
            value = load_case(workload)
            computed = workload["compute"]
            filesystem = workload["filesystem"]
            result = base.measure(workload["description"], value, compute_commands if computed else filesystem_commands if filesystem else commands,
                                  artifact / value.label, args, compute=computed, prepared_process=not computed and not filesystem,
                                  environment_override=process_api.environment())
            if computed and result["rounds"]:
                result["amortized_seconds"] = {name: item["median_seconds"] / result["rounds"] for name, item in result["summary"].items()}
            record["compute_workloads" if computed else "filesystem_workloads" if filesystem else "workloads"].append(result)
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
