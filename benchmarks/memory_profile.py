"""Profile preserved memory-report executables and verify complete Markdown.

Callgrind instruction counts and DHAT allocation traffic are diagnostics, not
wall-clock benchmark evidence. This adapter never builds or times a benchmark.
"""

import argparse
import hashlib
import json
import os
import platform
import re
import runpy
import shutil
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace

if sys.flags.optimize:
    raise SystemExit("profile validation requires Python assertions; do not use -O")

HERE = Path(__file__).resolve().parent
ENGINES = ("imre", "rust")
TOOLS = ("callgrind", "dhat")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def file_hash(path):
    return digest(path.read_bytes())


def save(path, value):
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")


def utc():
    return datetime.now(timezone.utc).isoformat()


def unique(values, label):
    if not values or len(values) != len(set(values)):
        raise ValueError(f"{label} must be nonempty and unique")


def tracer_pid():
    return int(next(line.split(":", 1)[1]
                    for line in Path("/proc/self/status").read_text().splitlines()
                    if line.startswith("TracerPid:")))


def inventory(files):
    return {name: {"sha256": digest(data), "bytes": len(data)}
            for name, data in sorted(files.items())}


def checked_case(case):
    """Only relative, ordinary files from the frozen benchmark recipe are used."""
    if not re.fullmatch(r"[A-Za-z0-9_-]+", case.label):
        raise ValueError("workload labels must be simple path components")
    if case.directories or case.hardlinks:
        raise ValueError("memory profile workloads use ordinary files only")
    for name in case.files:
        path = Path(name)
        if path.is_absolute() or ".." in path.parts or "\0" in name:
            raise ValueError("workload files must remain inside the retained cwd")
    if (len(case.arguments) != 6
            or tuple(case.arguments[::2]) != ("--old", "--new", "--output")
            or any(name not in case.files for name in case.arguments[1::2])
            or len(set(case.arguments[1::2])) != 3):
        raise ValueError("profile workloads need distinct old/new/output files")


def profiler(args):
    """Use the same Callgrind/DHAT flags as dependency_profile and hash tools."""
    executable = Path(args.valgrind or shutil.which("valgrind")
                      or Path.home() / ".cache/imre/profiling-tools/root/usr/bin/valgrind")
    executable = Path(shutil.which(str(executable)) or executable).resolve(strict=True)
    library = args.valgrind_lib or os.environ.get("VALGRIND_LIB")
    if library is None:
        library = next((executable.parent.parent / relative
                        for relative in ("libexec/valgrind", "lib/valgrind")
                        if (executable.parent.parent / relative).is_dir()), None)
    if library is None:
        raise ValueError("supply --valgrind-lib so the selected tools can be hashed")
    library = Path(library).resolve(strict=True)
    arch = {"x86_64": "amd64", "aarch64": "arm64"}.get(platform.machine(), platform.machine())
    paths = {"valgrind": executable,
             **{tool: library / f"{tool}-{arch}-linux" for tool in args.tools},
             **{path.name: path for path in sorted(library.glob(f"vgpreload_*-{arch}-linux.so"))}}
    environment = {**os.environ, "VALGRIND_OPTS": "", "VALGRIND_LIB": str(library),
                   "PYTHONHASHSEED": "0", "PYTHONPATH": ""}
    environment.pop("PYTHONHOME", None)
    return executable, paths, environment


def profile(args):
    for values, label in ((args.engine, "engines"), (args.workload, "workloads"),
                          (args.tools, "tools"), (args.copies, "copy counts")):
        unique(values, label)
    if args.timeout <= 0 or any(count <= 0 for count in args.copies):
        raise ValueError("timeout and copy counts must be positive")
    inputs, hashes = {}, {}

    def retain(name, path, expected=None):
        path = Path(path).resolve(strict=True)
        actual = file_hash(path)
        if expected is not None and actual != expected:
            raise ValueError(f"preserved input differs from the preparation: {name}")
        if name in inputs and (inputs[name] != path or hashes[name] != actual):
            raise ValueError(f"conflicting input identity: {name}")
        inputs[name], hashes[name] = path, actual
        return path

    def unchanged():
        for name, path in inputs.items():
            if file_hash(path) != hashes[name]:
                raise ValueError(f"input changed during profiling: {name}")

    retain("harness", Path(__file__))
    record_path = retain("record", args.record)
    record = json.loads(record_path.read_bytes())
    if record.get("kind") not in ("memory-preparation", "memory-benchmark"):
        raise ValueError("--record must be a memory preparation or benchmark record")
    if record.get("input_guards_passed") is not True:
        raise ValueError("the input record must have completed its input guards")
    measured = record["kind"] == "memory-benchmark"
    prepared = record["preparation"] if measured else record
    if (prepared.get("kind") != "memory-preparation"
            or prepared.get("input_guards_passed") is not True):
        raise ValueError("the record must contain a completed memory preparation")
    if measured and any(record[name] != prepared[name] for name in ("revision", "dirty")):
        raise ValueError("benchmark and preparation source identities differ")
    paths = prepared["source_and_tool_paths"]
    recorded_hashes = prepared["source_and_tool_sha256"]
    if paths.keys() != recorded_hashes.keys():
        raise ValueError("preparation paths and hashes differ")
    for name, path in paths.items():
        retain("prepared:" + name, path, recorded_hashes[name])
    for name in ("binary", "llvm", "validation"):
        retain(name, prepared[name], prepared[name + "_sha256"])
    binaries = {"imre": inputs["binary"], "rust": inputs["prepared:rust"]}
    rust_record = json.loads(inputs["prepared:rust_build_record"].read_bytes())
    if (rust_record != prepared["rust_build_record"]
            or rust_record.get("input_guards_passed") is not True
            or rust_record["binary_sha256"] != hashes["prepared:rust"]
            or rust_record["thinlto"] != prepared["thinlto"]):
        raise ValueError("Rust build provenance differs from the preparation")
    validation = json.loads(inputs["validation"].read_bytes())
    if (validation["cases"] != prepared["validation_cases"]
            or not set(ENGINES) <= set(validation["engines"])):
        raise ValueError("preparation validation does not cover both native engines")

    # Verify frozen generator/validator bytes before executing their pure helpers.
    for name in ("memory_benchmark.py", "memory_validate.py"):
        if inputs.get("prepared:helper:" + name) != (HERE / name).resolve():
            raise ValueError(f"preparation must bind this exact helper: {name}")
    driver_path = retain("parser", HERE / "dependency_profile.py")
    driver = SimpleNamespace(**runpy.run_path(str(driver_path)))
    benchmark = SimpleNamespace(**runpy.run_path(str(HERE / "memory_benchmark.py")))
    validator = benchmark.validator
    unchanged()
    if (benchmark.targets.elf_target(binaries["imre"]) != prepared["native_elf"]
            or benchmark.targets.elf_target(binaries["rust"]) != prepared["native_elf"]):
        raise ValueError("native executable targets differ from the preparation")
    available = benchmark.workloads(args.copies)
    by_label = {case.label: (metadata, case) for metadata, case in available}
    if len(by_label) != len(available) or set(args.workload) - by_label.keys():
        raise ValueError("selected workload is absent or duplicated in the frozen recipe")
    recorded_cases = {item["label"]: item for item in record.get("workloads", [])}
    if measured and len(recorded_cases) != len(record["workloads"]):
        raise ValueError("benchmark workload labels must be unique")
    cases = []
    for label in args.workload:
        metadata, case = by_label[label]
        checked_case(case)
        if metadata["label"] != label:
            raise ValueError("recipe label differs from its case")
        if measured and (label not in recorded_cases or any(
                recorded_cases[label].get(key) != value for key, value in metadata.items())):
            raise ValueError("regenerated workload differs from the benchmark record")
        expected = validator.oracle(case)
        if expected["error"] is not None:
            raise ValueError("profiles require successful complete-report workloads")
        if measured:
            row = recorded_cases[label]
            expected_files = inventory(validator.apply_output(case, expected, expected["rendered"]))
            for round_ in [row["gates"], *row["warmups"], *row["samples"]]:
                for engine in args.engine:
                    run = round_[engine]
                    command = run["command"]
                    if (not command or Path(command[0]).resolve() != binaries[engine]
                            or command[1:] != list(case.arguments)
                            or run["returncode"] != 0 or run["stdout"] or run["stderr"]
                            or run["files"] != expected_files or run["directories"] != expected["directories"]):
                        raise ValueError("recorded native commands or complete outputs differ from this workload")
        cases.append((metadata, case, expected))

    valgrind, profiler_files, environment = profiler(args)
    for name, path in profiler_files.items():
        retain("profiler:" + name, path)
    retain("python", Path(sys.executable))
    affinity = os.sched_getaffinity(0)
    cpu = min(affinity) if args.cpu is None else args.cpu
    if cpu not in affinity:
        raise ValueError("CPU must be in the process's allowed affinity")
    before = tracer_pid()
    if before:
        raise ValueError("profile harness must not be ptrace-traced")
    os.sched_setaffinity(0, {cpu})
    version = subprocess.check_output([str(valgrind), "--version"], env=environment,
                                      text=True, timeout=args.timeout).strip()
    output = args.output.absolute()
    if output.exists() or output.is_symlink():
        raise ValueError("profiling output must be a new directory")
    resolved = output.resolve()
    if any(path.is_relative_to(resolved) or resolved.is_relative_to(path)
           for path in inputs.values()):
        raise ValueError("profiling output overlaps an immutable input")
    output.mkdir(parents=True, exist_ok=False)
    output = output.resolve()
    (output / "record.json").write_bytes(record_path.read_bytes())
    source_dir = output / "sources"
    source_dir.mkdir()
    retained_sources = {}
    for index, (name, path) in enumerate(sorted(inputs.items())):
        if name in ("harness", "parser", "llvm", "validation") or name.startswith(
                ("prepared:helper:", "prepared:example:", "prepared:stdlib:",
                 "prepared:fixture:", "prepared:upstream:", "prepared:rust_source:")) or name.endswith("build_record"):
            destination = source_dir / f"{index:03d}-{path.name}"
            data = path.read_bytes()
            if digest(data) != hashes[name]:
                raise ValueError(f"input changed while retaining evidence: {name}")
            destination.write_bytes(data)
            retained_sources[name] = destination.relative_to(output).as_posix()
    workloads_dir = output / "workloads"
    workloads_dir.mkdir()
    selected = []
    for metadata, case, expected in cases:
        directory = workloads_dir / case.label
        directory.mkdir()
        (directory / "expected.md").write_bytes(expected["rendered"].encode())
        selected.append({"metadata": metadata, "arguments": list(case.arguments),
                         "input_files": inventory(case.files),
                         "expected_markdown_sha256": digest(expected["rendered"].encode()),
                         "expected_markdown_bytes": len(expected["rendered"].encode())})
    save(output / "workloads.json", selected)
    provenance = {
        "schema": 1, "kind": "memory-profile", "status": "running",
        "method": driver.METHOD, "started_at_utc": utc(),
        "association": "exact recorded benchmark workloads" if measured else
                       "exploratory preparation-only workloads; no timing association",
        "revision": prepared["revision"], "dirty": prepared["dirty"],
        "thinlto": prepared["thinlto"], "effective_target": prepared["effective_target"],
        "compile_command": prepared["compile_command"], "cpu": cpu,
        "profiler_version": version, "python_version": sys.version,
        "profiler_library": environment["VALGRIND_LIB"],
        "host": {"os": platform.system(), "architecture": platform.machine(),
                 "kernel": platform.release()},
        "tracer_pid_before": before,
        "perf_event_paranoid": Path("/proc/sys/kernel/perf_event_paranoid").read_text().strip(),
        "hardware_counters": "Not collected. Callgrind reports guest instructions, not CPU cycles or elapsed-time shares.",
        "environment": {name: environment.get(name) for name in (
            "VALGRIND_LIB", "VALGRIND_OPTS", "LANG", "LC_ALL", "LD_PRELOAD",
            "LD_LIBRARY_PATH", "MALLOC_ARENA_MAX", "MALLOC_PERTURB_", "GLIBC_TUNABLES")},
        "environment_scope": "Selected variables only; both engines inherit the same process environment.",
        "filesystem": "A fresh retained cwd per engine/tool. Parsing both input files and writing/closing the complete Markdown report are instrumented; fixture creation and validation are outside instrumentation. No cold-cache or fsync claim.",
        "inputs": {name: {"path": str(path), "sha256": hashes[name], "bytes": path.stat().st_size}
                   for name, path in inputs.items()},
        "retained_sources": retained_sources, "workloads": selected, "runs": [],
        "input_guards_passed": False,
    }
    summary = {"schema": 1, "kind": "memory-profile-summary", "method": driver.METHOD,
               "runs": []}
    save(output / "provenance.json", provenance)
    try:
        for metadata, case, expected in cases:
            for engine in args.engine:
                for tool in args.tools:
                    unchanged()
                    label = f"{case.label}.{engine}.{tool}"
                    directory = output / "files" / label
                    directory.mkdir(parents=True)
                    validator.materialize(case, directory)
                    trace, log = output / (label + ".trace"), output / (label + ".log")
                    command = [str(valgrind), "--command-line-only=yes", f"--tool={tool}",
                               "--num-callers=24", f"--log-file={log}"]
                    command += (["--dump-instr=yes", "--collect-jumps=yes",
                                 f"--callgrind-out-file={trace}"] if tool == "callgrind"
                                else ["--mode=heap", f"--dhat-out-file={trace}"])
                    command += [str(binaries[engine]), *case.arguments]
                    start, started = time.perf_counter_ns(), utc()
                    timed_out = False
                    try:
                        process = subprocess.run(command, input=b"", capture_output=True,
                                                 cwd=directory, env=environment,
                                                 timeout=args.timeout, check=False)
                    except subprocess.TimeoutExpired as error:
                        timed_out = True
                        process = subprocess.CompletedProcess(command, -1, error.stdout or b"", error.stderr or b"")
                    elapsed = time.perf_counter_ns() - start
                    for stream in ("stdout", "stderr"):
                        (output / (label + "." + stream)).write_bytes(getattr(process, stream))
                    item = {"workload": case.label, "engine": engine, "tool": tool,
                            "command": command, "cwd": directory.relative_to(output).as_posix(),
                            "started_at_utc": started, "completed_at_utc": utc(),
                            "diagnostic_elapsed_ns": elapsed, "timed_out": timed_out,
                            "returncode": process.returncode,
                            "binary_sha256": file_hash(binaries[engine]),
                            "stdout_sha256": digest(process.stdout), "stdout_bytes": len(process.stdout),
                            "stderr_sha256": digest(process.stderr), "stderr_bytes": len(process.stderr),
                            "trace": trace.name, "trace_sha256": file_hash(trace) if trace.exists() else None,
                            "log": log.name, "log_sha256": file_hash(log) if log.exists() else None,
                            "complete_output_verified": False}
                    provenance["runs"].append(item)
                    save(output / "provenance.json", provenance)
                    if timed_out or not trace.is_file() or not log.is_file():
                        raise RuntimeError(f"profile failed: {label}; partial evidence retained")
                    item["result"] = validator.check_result(case, expected, process, directory, engine)
                    item["complete_output_verified"] = True
                    unchanged()
                    values = driver.callgrind(trace) if tool == "callgrind" else driver.dhat(trace)
                    summary["runs"].append({"workload": case.label, "engine": engine, "tool": tool,
                                            "trace_sha256": item["trace_sha256"], **values})
                    save(output / "summary.json", summary)
                    save(output / "provenance.json", provenance)
                    counts = (f"{values['total_instructions']:,} instructions" if tool == "callgrind"
                              else f"{values['totals']['tbk']:,} allocated blocks, {values['totals']['tb']:,} requested bytes")
                    print(f"Verified {label}: {counts}", flush=True)
        unchanged()
        provenance["tracer_pid_after"] = tracer_pid()
        if provenance["tracer_pid_after"] or os.sched_getaffinity(0) != {cpu}:
            raise RuntimeError("profile harness tracing or affinity changed")
        provenance.update(status="passed", input_guards_passed=True, completed_at_utc=utc())
    except BaseException as error:
        provenance.update(status="failed", failure=f"{type(error).__name__}: {error}", completed_at_utc=utc())
        raise
    finally:
        save(output / "provenance.json", provenance)
        save(output / "manifest.json", {
            path.relative_to(output).as_posix(): {"bytes": path.stat().st_size, "sha256": file_hash(path)}
            for path in sorted(output.rglob("*")) if path.is_file() and path != output / "manifest.json"})
    print(f"Saved {output}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--record", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--workload", action="append", required=True)
    parser.add_argument("--copies", type=int, nargs="+", default=[32, 256])
    parser.add_argument("--engine", choices=ENGINES, action="append")
    parser.add_argument("--tools", choices=TOOLS, nargs="+", default=list(TOOLS))
    parser.add_argument("--valgrind")
    parser.add_argument("--valgrind-lib", type=Path)
    parser.add_argument("--cpu", type=int)
    parser.add_argument("--timeout", type=float, default=600)
    args = parser.parse_args()
    args.engine = args.engine or list(ENGINES)
    profile(args)


if __name__ == "__main__":
    main()
