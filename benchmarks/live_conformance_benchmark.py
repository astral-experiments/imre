"""Five-engine live conformance timings, separately from frozen report compute."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import runpy
import stat
import statistics
import subprocess
import sys
import time
from types import SimpleNamespace


ROOT = Path(__file__).resolve().parents[1]
HERE = ROOT / "benchmarks"
live = SimpleNamespace(**runpy.run_path(str(HERE / "live_conformance_validate.py")))
offline = SimpleNamespace(**runpy.run_path(str(HERE / "conformance_validate.py")))
shared = SimpleNamespace(**runpy.run_path(str(HERE / "native_tools_benchmark.py")))
prepare = runpy.run_path(str(ROOT / "examples/conformance_report/prepare.py"))["prepare"]


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def identity(path: Path) -> dict:
    path = path.absolute()
    return {"path": str(path), "resolved_path": str(path.resolve(strict=True)),
            "symlink": os.readlink(path) if path.is_symlink() else None,
            "sha256": digest(path), "bytes": path.stat().st_size,
            "mode": stat.S_IMODE(path.stat().st_mode)}


def verify_build_record(path: Path, guarded: dict[str, dict]) -> dict:
    record = json.loads(path.read_text())
    assert record.get("status", "passed") == "passed", (path, "build did not pass")
    assert record.get("input_guards_passed", "source_inputs" in record), (path, "build input guards failed")

    def guard(file: str | Path, expected: str) -> None:
        file = Path(file).absolute()
        item = identity(file)
        assert item["sha256"] == expected, (file, "retained build input/output differs")
        guarded[str(file)] = item

    for name, expected in record.get("tool_sha256", {}).items():
        guard(record["tool_paths"][name], expected)
    if record.get("kind") == "native-tools-build":
        for name, expected in record["source_sha256"].items():
            guard(ROOT / name, expected)
            guard(path.parent / "sources" / name, expected)
        guard(HERE / "native_tools_build.py", record["driver_sha256"])
        guard(path.parent / "build.py", record["driver_sha256"])
        for name, expected in record["sha256"].items():
            guard(path.parent / ("relar" if name == "compiler" else name + ".a"), expected)
        for step in record["steps"]:
            assert step["returncode"] == 0
            for name in ("log", "messages"):
                guard(step[name], step[name + "_sha256"])
    elif record.get("kind") == "live-conformance-program-build":
        assert record["input_guards_passed"] and record["status"] == "passed"
        core = record["core_build_record"]
        guard(core["path"], core["sha256"])
        core_record = verify_build_record(Path(core["path"]), guarded)
        assert record["compiler"]["sha256"] == core_record["sha256"]["compiler"]
        assert record["runtime"]["sha256"] == core_record["sha256"][record["mode"] + "_runtime"]
        assert record["target"] == core_record["target"]
        for name, expected in record["source_inputs"].items():
            guard(name, expected)
        for item in [record["compiler"], record["runtime"], *record["tools"].values(),
                     *record["retained_sources"].values(), *record["binaries"].values(), *record["llvm"].values()]:
            assert identity(Path(item["path"])) == item, (item["path"], "program build identity differs")
            guard(item["path"], item["sha256"])
        for name, item in record["input_identities"].items():
            assert identity(Path(name)) == item, (name, "program build input differs")
            guarded[name] = item
        for step in record["steps"]:
            assert step["returncode"] == 0
            command = step["command"]
            assert command[0] == record["compiler"]["path"] and command[1] == "build"
            entry = "compute.py" if step["name"].endswith("-compute") else "__main__.py"
            assert command[2] == str(ROOT / "examples/conformance_report" / entry)
            for flag, expected in (("--runtime", record["runtime"]["path"]),
                                   ("--clang", record["tools"]["clang"]["path"]),
                                   ("--module-root", str(ROOT / "examples")), ("--target", record["target"]),
                                   ("--opt-level", "3"), ("-o", record["binaries"][step["name"]]["path"]),
                                   ("--emit-llvm", record["llvm"][step["name"]]["path"])):
                assert command[command.index(flag) + 1] == expected
            assert ("--sanitize" in command) == (record["mode"] == "asan")
            assert ("--lto-linker" in command) == (record["mode"] == "thinlto")
            if record["mode"] == "thinlto":
                assert command[command.index("--lto-linker") + 1] == record["tools"]["lto_linker"]["path"]
            for stream in ("stdout", "stderr"):
                guard(step[stream]["path"], step[stream]["sha256"])
        for binary, expected in record["linked_libraries"].items():
            assert shared.linked_libraries(Path(binary)) == expected, (binary, "program runtime differs")
    elif "source_inputs" in record:
        assert record["status"] == "passed" and record["release"] == "v0.20.4"
        assert record["domain"]["python_interop"] is False
        guard(record["compiler"], record["compiler_sha256"])
        for name, expected in record["source_inputs"].items():
            guard(name, expected)
        for name, expected in record["generated"].items():
            guard(Path(record["environment_overrides"]["CODON_PATH"]) / name, expected)
        for item in [*record["libraries"].values(), *record["binaries"].values(),
                     record["native_json"], record["native_platform"]]:
            guard(item["path"], item["sha256"])
        for step in record["commands"]:
            assert step["returncode"] == 0
            assert all(flag in step["command"] for flag in ("-release", "-numerics=py", "-disable-native", "-exe"))
            for stream in ("stdout", "stderr"):
                guard(step[stream]["path"], step[stream]["sha256"])
    elif record.get("kind") == "native-benchmark-platform-build":
        for name, expected in record["sources"].items():
            guard(HERE / "native_platform" / name, expected)
        guard(record["library"]["path"], record["library"]["sha256"])
    elif record.get("entrypoint") == "conformance_report":
        for name, expected in record["sources"].items():
            guard(HERE / "reference/conformance_report" / name, expected)
        guard(path.parent / "conformance-report-reference", record["binary_sha256"])
    else:
        raise ValueError("unrecognized build record: " + str(path))
    return record


def timed(command: list[str], arguments: list[str], directory: Path, expected_stdout: bytes, expected_report: bytes) -> dict:
    output = Path(arguments[arguments.index("--output") + 1])
    output.write_bytes(live.SENTINEL)
    before = live.tree_identity(directory)
    environment = {**live.environment(directory / "home"), "PYTHONPATH": ""}
    started = time.perf_counter_ns()
    completed = live.run_process([*command, *arguments], directory=directory, env=environment)
    elapsed = time.perf_counter_ns() - started
    assert completed.returncode == 0, (command, completed.stderr.decode(errors="replace"))
    assert completed.stderr == b"", (command, completed.stderr)
    assert completed.stdout == expected_stdout, (command, "stdout differs")
    assert output.is_file() and not output.is_symlink(), (command, "output is not a regular file")
    assert output.read_bytes() == expected_report, (command, "full report differs")
    after = live.tree_identity(directory)
    name = output.relative_to(directory).as_posix()
    before.pop(name)
    after.pop(name)
    assert after == before, (command, "unexpected filesystem effects")
    return {"elapsed_ns": elapsed, "stdout_sha256": hashlib.sha256(completed.stdout).hexdigest(),
            "report_sha256": digest(output), "report_bytes": len(expected_report),
            "returncode": completed.returncode, "filesystem_guards_passed": True}


def main() -> None:
    if sys.flags.optimize:
        raise SystemExit("benchmark validation requires assertions")
    sys.dont_write_bytecode = True
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", required=True, type=Path)
    parser.add_argument("--old-ty", required=True, type=Path)
    parser.add_argument("--new-ty", required=True, type=Path)
    parser.add_argument("--python", default=sys.executable, type=Path)
    parser.add_argument("--relar", required=True, type=Path)
    parser.add_argument("--relar-compute", required=True, type=Path)
    parser.add_argument("--codon", required=True, type=Path)
    parser.add_argument("--codon-compute", required=True, type=Path)
    parser.add_argument("--rust", required=True, type=Path)
    parser.add_argument("--build-record", action="append", required=True, type=Path,
                        help="supply Relar-program, Rust-reference, Codon, and native-platform build records")
    parser.add_argument("--old-ty-version", default="0.0.60")
    parser.add_argument("--new-ty-version", default="0.0.90")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--samples", type=int, default=10)
    parser.add_argument("--warmups", type=int, default=1)
    parser.add_argument("--compute-rounds", type=int, default=4)
    parser.add_argument("--cpu", type=int)
    args = parser.parse_args()
    if args.samples <= 0 or args.samples % 10 or args.warmups < 0 or args.compute_rounds <= 0:
        parser.error("samples must be a positive multiple of 10, warmups nonnegative, and compute rounds positive")
    if sys.gettrace() is not None or sys.getprofile() is not None:
        raise RuntimeError("timing with tracing/profiling enabled is unsupported")
    affinity = sorted(os.sched_getaffinity(0))
    cpu = affinity[0] if args.cpu is None else args.cpu
    if cpu not in affinity:
        parser.error("selected CPU is outside this process's affinity")
    os.sched_setaffinity(0, {cpu})
    for field in ("suite", "old_ty", "new_ty", "relar", "relar_compute", "codon", "codon_compute", "rust"):
        setattr(args, field, getattr(args, field).resolve(strict=True))
    # Preserve the interpreter's symlink spelling so pyvenv.cfg remains active.
    args.python = args.python.absolute()
    assert args.python.is_file()
    args.build_record = [path.resolve(strict=True) for path in args.build_record]
    source_revision = subprocess.check_output(["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True).strip()
    revision = subprocess.check_output(["git", "-C", str(args.suite), "rev-parse", "HEAD"], text=True).strip()
    checkout = subprocess.check_output(["git", "-C", str(args.suite), "rev-parse", "--show-toplevel"], text=True).strip()
    assert revision == live.REVISION, "live benchmark requires the pinned typing checkout"
    assert args.suite == Path(checkout) / "conformance", "suite must be the pinned checkout's conformance directory"
    assert subprocess.check_output(["git", "-C", checkout, "rev-parse", "--is-shallow-repository"], text=True).strip() == "false", "typing checkout must be complete"
    assert not subprocess.check_output(["git", "-C", checkout, "status", "--porcelain"], text=True), "typing checkout must start clean"
    artifact = args.output.resolve()
    assert not artifact.is_relative_to(Path(checkout)), "benchmark artifacts must not modify the typing checkout"
    artifact.mkdir(parents=True, exist_ok=False)
    commands = {"original": live.original_command(args.python), "port": offline.python_command(args.python),
                "relar": [str(args.relar)], "codon": [str(args.codon)], "rust": [str(args.rust)]}
    compute_commands = {"original": offline.original_compute_command(args.python), "port": offline.python_compute_command(args.python),
                        "relar": [str(args.relar_compute)], "codon": [str(args.codon_compute)], "rust": [str(args.rust), "--compute"]}
    guarded = {str(path): identity(path) for path in (args.old_ty, args.new_ty, args.python, args.relar,
                args.relar_compute, args.codon, args.codon_compute, args.rust, *args.build_record)}
    build_records = {str(path): verify_build_record(path.resolve(strict=True), guarded) for path in args.build_record}
    records_by_kind = {}
    for record in build_records.values():
        kind = record.get("kind", "rust" if record.get("entrypoint") == "conformance_report" else "codon")
        assert kind not in records_by_kind, (kind, "duplicate build record")
        records_by_kind[kind] = record
        if kind == "live-conformance-program-build":
            assert record["source_matches_revision"], "timings require committed program inputs"
            assert record["mode"] in ("normal", "thinlto"), "sanitizer builds are validation-only"
            assert digest(args.relar) == record["binaries"]["conformance_report"]["sha256"]
            assert digest(args.relar_compute) == record["binaries"]["conformance_report-compute"]["sha256"]
        elif kind == "codon":
            assert digest(args.codon) == record["binaries"]["conformance_report"]["sha256"]
            assert digest(args.codon_compute) == record["binaries"]["conformance_report-compute"]["sha256"]
        elif record.get("entrypoint") == "conformance_report":
            assert digest(args.rust) == record["binary_sha256"]
    required = {"live-conformance-program-build", "codon", "rust", "native-benchmark-platform-build"}
    assert required <= records_by_kind.keys(), ("missing required build provenance", required - records_by_kind.keys())
    program, rust, platform, codon = (records_by_kind[name] for name in
                                     ("live-conformance-program-build", "rust", "native-benchmark-platform-build", "codon"))
    thinlto = program["mode"] == "thinlto"
    assert program["target"] == rust["target"] == platform["target"], "native build targets differ"
    for record in (rust, platform):
        settings = record["environment"]
        assert record["thinlto"] == thinlto, "native release/ThinLTO modes differ"
        assert settings["CARGO_PROFILE_RELEASE_OPT_LEVEL"] == "3"
        assert settings["CARGO_PROFILE_RELEASE_CODEGEN_UNITS"] == "16"
        assert settings["CARGO_PROFILE_RELEASE_LTO"] == ("thin" if thinlto else "off")
        assert settings["RUSTC_WRAPPER"] == settings["RUSTC_WORKSPACE_WRAPPER"] == ""
        assert settings["RUSTC"] == record["tool_paths"]["rustc"]
        assert record["command"][record["command"].index("--target") + 1] == program["target"]
    assert codon["native_platform"]["sha256"] == platform["library"]["sha256"], "Codon platform provenance differs"
    libraries = {}
    for binary in (args.python, args.old_ty, args.new_ty, args.relar, args.relar_compute, args.codon, args.codon_compute, args.rust):
        libraries[str(binary)] = shared.linked_libraries(binary)
        for path in libraries[str(binary)]:
            guarded[path] = identity(Path(path))
        if binary != args.python:
            assert not any("libpython" in Path(path).name.lower() for path in libraries[str(binary)]), "native engine links Python"
    for binary in (args.codon, args.codon_compute):
        required_libraries = [codon["libraries"]["libcodonrt.so"]]
        if binary == args.codon:
            required_libraries.append(codon["native_platform"])
        for item in required_libraries:
            dependency = str(Path(item["path"]).resolve(strict=True))
            assert dependency in libraries[str(binary)], (binary, "recorded Codon runtime is not linked", dependency)
            assert libraries[str(binary)][dependency]["sha256"] == item["sha256"]
    assert len({tuple(shared.elf_target(path).items()) for path in
                (args.relar, args.relar_compute, args.codon, args.codon_compute, args.rust)}) == 1
    runtime_environment = {**live.environment(artifact / "python-home"), "PYTHONPATH": ""}
    python_info = json.loads(subprocess.check_output([
        str(args.python), "-s", "-B", "-c", shared.shared.PYTHON_INFO,
    ], env=runtime_environment, text=True))
    assert python_info["implementation"] == "cpython" and tuple(map(int, python_info["version_number"].split(".")[:2])) >= (3, 14)
    assert not python_info["jit_enabled"], "CPython JIT must be disabled"
    python_runtime = json.loads(subprocess.check_output([
        str(args.python), "-I", "-B", "-c",
        "import importlib,json,pathlib,sys,sysconfig; parser=importlib.import_module('tomllib' if sys.version_info >= (3,15) else 'tomli'); "
        "print(json.dumps({'prefix':sys.prefix,'base_prefix':sys.base_prefix,'toml_parser':parser.__name__,'toml_version':getattr(parser,'__version__','stdlib'),"
        "'stdlib':sysconfig.get_path('stdlib'),'platstdlib':sysconfig.get_path('platstdlib'),'stdlib_zip':[p for p in sys.path if p.endswith('.zip') and pathlib.Path(p).is_file()],"
        "'toml_files':[str(p) for p in pathlib.Path(parser.__file__).parent.rglob('*') "
        "if p.is_file()]}))",
    ], env=runtime_environment, text=True))
    python_info.update(python_runtime)
    for name in [*python_info["toml_files"], *python_info["stdlib_zip"]]:
        guarded[name] = identity(Path(name))
    for name in {python_info["stdlib"], python_info["platstdlib"]}:
        for path in Path(name).rglob("*"):
            if path.is_file() and path.suffix in {".py", ".pyc", ".so"} and "site-packages" not in path.parts:
                guarded[str(path)] = identity(path)
    for prefix in {Path(python_info["prefix"]), args.python.parent.parent}:
        path = prefix / "pyvenv.cfg"
        if path.is_file():
            guarded[str(path)] = identity(path)
    ty_versions = {}
    for name, binary, version in (("old", args.old_ty, args.old_ty_version), ("new", args.new_ty, args.new_ty_version)):
        completed = live.run_process([str(binary), "--version"], directory=artifact, env=runtime_environment)
        assert completed.returncode == 0 and not completed.stderr, (binary, completed.stderr)
        actual = completed.stdout.decode().strip()
        assert actual == "ty " + version or actual.startswith("ty " + version + " "), (name, actual, "unexpected ty version")
        ty_versions[name] = actual
    assert digest(args.old_ty) != digest(args.new_ty), "changed workload needs distinct ty binaries"
    source_directories = (ROOT / "examples/conformance_report", ROOT / "stdlib/relar", HERE / "codon_native",
                          HERE / "reference/conformance_report", HERE / "native_platform")

    def source_files() -> list[Path]:
        return sorted(path for directory in source_directories for path in directory.rglob("*")
                      if path.is_file() and "target" not in path.parts and
                      (path.suffix in {".py", ".pyc", ".rs", ".toml", ".lock", ".codon"}
                       or path.name in {"LICENSE", "UPSTREAM.json"}))

    source_membership = source_files()
    for path in source_membership:
        guarded[str(path)] = identity(path)
    helpers = (Path(__file__), HERE / "live_conformance_validate.py", HERE / "conformance_validate.py",
               HERE / "native_tools_benchmark.py", HERE / "benchmark.py", HERE / "live_conformance_build.py")
    for path in helpers:
        guarded[str(path)] = identity(path)
    source_hashes = {str(path): guarded[str(path)]["sha256"] for path in [*source_membership, *helpers]
                     if path.suffix in {".py", ".rs", ".toml", ".lock"}}
    for name, expected in source_hashes.items():
        relative = Path(name).relative_to(ROOT).as_posix()
        committed = subprocess.run(["git", "-C", str(ROOT), "show", source_revision + ":" + relative], capture_output=True)
        assert committed.returncode == 0 and hashlib.sha256(committed.stdout).hexdigest() == expected, (relative, "measured source differs from HEAD")
    suite_before = live.tree_identity(args.suite)
    validation = live.validate(commands, args.suite, args.old_ty, args.new_ty, artifact / "validation", args.python)
    (artifact / "validation.json").write_text(json.dumps(validation, indent=2) + "\n")
    workloads = []
    frozen_inputs = {}
    for name, old, new, format in (("unchanged-github", args.new_ty, args.new_ty, "github"),
                                   ("changed-github", args.old_ty, args.new_ty, "github"),
                                   ("changed-diff", args.old_ty, args.new_ty, "diff")):
        directory = artifact / name
        directory.mkdir()
        captures = directory / "captures"
        preparation = prepare(args.suite, old, new, captures, live.REVISION)
        assert (preparation["captures"]["old"]["stdout_sha256"] == preparation["captures"]["new"]["stdout_sha256"]) == name.startswith("unchanged"), "workload diagnostic changes differ from label"
        frozen_inputs[str(captures)] = live.tree_identity(captures)
        workload = offline.Workload(captures / "manifest.json", captures / "old.json", captures / "new.json", format, True)
        expected = offline.oracle(workload)
        assert expected.error is None and expected.report is not None, expected
        report = expected.report.encode("utf-8")
        output_dir = directory / "output"
        output_dir.mkdir()
        (output_dir / "home").mkdir()
        output = output_dir / "report.md"
        workloads.append({"name": name + "/live", "kind": "live", "commands": commands,
                          "arguments": live.live_arguments(args.suite, old, new, output, format=format),
                          "directory": output_dir, "stdout": b"", "report": report,
                          "preparation": preparation})
        for rounds in (0, args.compute_rounds):
            workloads.append({"name": name + f"/compute-{rounds}", "kind": "report-compute",
                              "commands": compute_commands,
                              "arguments": [*workload.arguments(), "--rounds", str(rounds), "--output", str(output)],
                              "directory": output_dir, "stdout": f"{len(expected.report) * rounds}\n".encode(),
                              "report": report if rounds else b"", "preparation": preparation})
    rows = []
    orders = shared.engine_orders()
    progress = {"schema": 1, "kind": "live-conformance-benchmark", "status": "running",
                "input_guards_passed": False, "source_revision": source_revision,
                "samples": args.samples, "warmups": args.warmups, "orders": orders,
                "rows_file": str(artifact / "samples.jsonl"),
                "workloads": [item["name"] for item in workloads]}
    (artifact / "benchmark.json").write_text(json.dumps(progress, indent=2) + "\n")
    with (artifact / "samples.jsonl").open("x", encoding="utf-8") as samples_file:
        try:
            for workload in workloads:
                print("Timing " + workload["name"], flush=True)
                for round_index in range(-args.warmups, args.samples):
                    order = orders[round_index % len(orders)]
                    for position, engine in enumerate(order):
                        result = timed(workload["commands"][engine], workload["arguments"], workload["directory"],
                                       workload["stdout"], workload["report"])
                        row = {"workload": workload["name"], "kind": workload["kind"], "engine": engine,
                               "round": round_index, "position": position, "warmup": round_index < 0, **result}
                        rows.append(row)
                        samples_file.write(json.dumps(row) + "\n")
                        samples_file.flush()
        except BaseException as error:
            progress.update(status="failed", completed_rows=len(rows), error=str(error))
            (artifact / "benchmark.json").write_text(json.dumps(progress, indent=2) + "\n")
            raise
    summaries = []
    for workload in workloads:
        by_engine = {}
        for engine in shared.ENGINES:
            samples = [row["elapsed_ns"] / 1e9 for row in rows if row["workload"] == workload["name"]
                       and row["engine"] == engine and not row["warmup"]]
            by_engine[engine] = {"samples": len(samples), "median_seconds": statistics.median(samples),
                                 "minimum_seconds": min(samples), "maximum_seconds": max(samples)}
        summaries.append({"workload": workload["name"], "kind": workload["kind"], "engines": by_engine})
    assert live.tree_identity(args.suite) == suite_before, "typing checkout changed during benchmark"
    assert all(live.tree_identity(Path(path)) == expected for path, expected in frozen_inputs.items()), "frozen compute inputs changed"
    assert subprocess.check_output(["git", "-C", checkout, "rev-parse", "HEAD"], text=True).strip() == revision
    assert not subprocess.check_output(["git", "-C", checkout, "status", "--porcelain"], text=True), "typing checkout no longer clean"
    assert subprocess.check_output(["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True).strip() == source_revision
    assert all(identity(Path(path)) == item for path, item in guarded.items()), "guarded benchmark input changed"
    assert source_files() == source_membership, "measured source membership changed"
    assert all(shared.linked_libraries(Path(binary)) == expected for binary, expected in libraries.items()), "linked runtime resolution changed"
    result = {"schema": 1, "kind": "live-conformance-benchmark", "status": "passed",
              "created_at": datetime.now(timezone.utc).isoformat(), "cpu": cpu,
              "source_revision": source_revision, "source_matches_revision": True,
              "original_affinity": affinity, "orders": orders, "samples": args.samples,
              "warmups": args.warmups, "compute_rounds": args.compute_rounds,
              "timing_contract": {"live": "startup, imports, TOML/discovery, two actual ty children, path resolution, full report, atomic publication and cleanup",
                                  "report-compute": "frozen-input startup and parsing plus requested report rounds; no analyzer child execution",
                                  "compute-zero": "startup and frozen input loading baseline; do not subtract from live timings"},
              "method": "Fresh whole processes on one pinned CPU with warm filesystem caches. Complete-byte and filesystem guards run outside timing. Ten Williams orders balance positions and directed within-round neighbors; warmups and cross-round transitions are not balanced. Frozen compute-zero controls are reported separately without subtracting them from live or compute timings.",
              "commands": commands, "compute_commands": compute_commands,
              "build_records": build_records, "linked_libraries": libraries, "python": python_info,
              "ty_versions": ty_versions, "environment": {**live.environment(Path("<workload>/home")), "PYTHONPATH": ""},
              "native_boundary": "Codon uses its pinned official runtime and an independently built Rust library for TOML, captured child processes/timeouts, and temporary directories. The Rust reference shares this OS/TOML crate; report computation is independently implemented. No native engine links libpython.",
              "inputs": guarded, "python_underlying_binary": identity(args.python.resolve(strict=True)),
              "suite": str(args.suite), "suite_revision": revision, "suite_checkout": checkout,
              "suite_files": suite_before, "validation_sha256": digest(artifact / "validation.json"),
              "samples_sha256": digest(artifact / "samples.jsonl"),
              "frozen_inputs": frozen_inputs,
              "workloads": [{"name": item["name"], "arguments": item["arguments"], "preparation": item["preparation"]} for item in workloads],
              "rows": rows, "summaries": summaries, "input_guards_passed": True}
    (artifact / "benchmark.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"status": result["status"], "samples": len(rows), "output": str(artifact / "benchmark.json")}))


if __name__ == "__main__":
    main()
