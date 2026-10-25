"""Profile exact inventory executables against captured filesystem workloads.

Callgrind instruction counts and DHAT requested heap allocations are diagnostics,
not hardware cycles or wall-clock benchmark results. This driver never builds.
"""

import argparse
import json
import os
import platform
import re
import runpy
import shutil
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
driver = SimpleNamespace(**runpy.run_path(str(HERE / "dependency_profile.py")))
shared = SimpleNamespace(**runpy.run_path(str(HERE / "benchmark.py")))
validator = SimpleNamespace(**runpy.run_path(str(HERE / "inventory_validate.py")))
ENGINES = ("imre", "rust_bigint", "baseline")
DEFAULT_ENGINES = ("imre", "rust_bigint")
WORKLOADS = ("dense-16384", "sparse-16384")
HELPERS = (
    "inventory_profile.py",
    "dependency_profile.py",
    "benchmark.py",
    "validate.py",
    "inventory_validate.py",
    "manifest_validate.py",
    "batch_validate.py",
    "incremental_validate.py",
)


def save(path: Path, value: dict) -> None:
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")


def tracer_pid() -> int:
    return int(
        next(
            line.split(":", 1)[1]
            for line in Path("/proc/self/status").read_text().splitlines()
            if line.startswith("TracerPid:")
        )
    )


def captured(workload: dict):
    case = validator.case_from_record(workload["case"])
    if not re.fullmatch(r"[A-Za-z0-9_-]+", case.label):
        raise ValueError("captured workload labels must be simple path components")
    if any(
        Path(name).is_absolute() or ".." in Path(name).parts or "\0" in name
        for name in (
            *case.arguments,
            *case.files,
            *case.directories,
            *case.hardlinks,
            *case.hardlinks.values(),
        )
    ):
        raise ValueError("profiled file paths must remain inside the fixture")
    expected = validator.oracle(case)
    changed = {
        name: content
        for name, content in expected["files"].items()
        if case.files.get(name) != content
    }
    inventory = {
        name: {"sha256": validator.digest(content), "bytes": len(content)}
        for name, content in sorted(expected["files"].items())
    }
    if (
        workload["label"] != case.label
        or expected["returncode"] != 0
        or workload["expected_stdout"] != expected["stdout"]
        or workload["expected_changed_files"] != validator.encoded_files(changed)
        or workload["expected_file_inventory"] != inventory
    ):
        raise ValueError("captured workload differs from the independent oracle")
    return case, expected


def profiler(args):
    executable = Path(
        args.valgrind
        or shutil.which("valgrind")
        or Path.home() / ".cache/imre/profiling-tools/root/usr/bin/valgrind"
    )
    executable = Path(shutil.which(str(executable)) or executable).resolve(strict=True)
    library = args.valgrind_lib or os.environ.get("VALGRIND_LIB")
    if library is None:
        library = next(
            (
                executable.parent.parent / relative
                for relative in ("libexec/valgrind", "lib/valgrind")
                if (executable.parent.parent / relative).is_dir()
            ),
            None,
        )
    if library is None:
        raise ValueError("supply --valgrind-lib so profiler tools can be hashed")
    library = Path(library).resolve(strict=True)
    arch = {"x86_64": "amd64", "aarch64": "arm64"}.get(
        platform.machine(), platform.machine()
    )
    files = {"valgrind": executable}
    files.update({tool: library / f"{tool}-{arch}-linux" for tool in args.tools})
    files.update(
        {
            path.name: path
            for path in sorted(library.glob(f"vgpreload_*-{arch}-linux.so"))
        }
    )
    environment = {**os.environ, "VALGRIND_OPTS": "", "VALGRIND_LIB": str(library)}
    return executable, files, environment


def profile(args) -> None:
    cpu = shared.pin_cpu(args.cpu)
    before = tracer_pid()
    record_path = args.record.resolve(strict=True)
    record = json.loads(record_path.read_text())
    if record.get("example") != "inventory_reconciliation":
        raise ValueError("--record must identify an inventory comparison")
    engines = args.engines or list(DEFAULT_ENGINES)
    labels = args.workloads or list(WORKLOADS)
    for values in (engines, labels, args.tools):
        if not values or len(set(values)) != len(values):
            raise ValueError("engine, workload, and profiler selections must be unique")
    overrides = driver.unique_paths(args.binary, "--binary")
    builds = driver.unique_paths(args.build_record, "--build-record")
    if (overrides.keys() | builds.keys()) - set(engines):
        raise ValueError("binary/build overrides must name a selected engine")
    by_label = {row["label"]: row for row in record["workloads"]["inventory"]}
    if (
        len(by_label) != len(record["workloads"]["inventory"])
        or set(labels) - by_label.keys()
    ):
        raise ValueError("workload labels must identify unique captured cases")
    workloads = [(by_label[label], *captured(by_label[label])) for label in labels]
    inputs = {"comparison": record_path}
    hashes = {"comparison": shared.sha256(record_path)}

    def retain(name: str, path: Path, expected: str | None = None) -> Path:
        path = path.resolve(strict=True)
        value = shared.sha256(path)
        if expected is not None and value != expected:
            raise ValueError(f"preserved input differs from the comparison: {name}")
        inputs[name], hashes[name] = path, value
        return path

    commands = record["method"]["commands"]["inventory"]
    if set(engines) - commands.keys():
        raise ValueError("selected engine is absent from the comparison")
    engine_records = {engine: record for engine in engines}
    if "baseline" in engines:
        if args.baseline_record is None:
            raise ValueError("--engine baseline requires --baseline-record")
        path = retain(
            "baseline_comparison",
            args.baseline_record,
            record["sha256"]["inventory_baseline_record"],
        )
        baseline = json.loads(path.read_text())
        if (
            baseline.get("example") != "inventory_reconciliation"
            or baseline["sha256"]["inventory_binary"]
            != record["sha256"]["inventory_baseline"]
            or {
                name: digest
                for name, digest in baseline["sources"].items()
                if name.startswith("inventory_reconciliation/")
            }
            != {
                name: digest
                for name, digest in record["sources"].items()
                if name.startswith("inventory_reconciliation/")
            }
            or any(
                baseline["options"][name] != record["options"][name]
                for name in ("target", "lto", "llvm_budget")
            )
        ):
            raise ValueError("baseline comparison source, binary, or mode differs")
        engine_records["baseline"] = baseline
    elif args.baseline_record is not None:
        raise ValueError("--baseline-record requires --engine baseline")
    binary_keys = {
        "imre": "inventory_binary",
        "rust_bigint": "rust_bigint",
        "baseline": "inventory_baseline",
    }
    binaries = {
        engine: retain(
            "binary:" + engine,
            overrides.get(engine, Path(commands[engine][0])),
            record["sha256"][binary_keys[engine]],
        )
        for engine in engines
    }
    for engine in engines:
        selected_record = engine_records[engine]
        compile_command = selected_record["method"]["compile_commands"]["inventory"]
        native = engine in ("imre", "baseline")
        default = (
            Path(compile_command[0]).parent / "build-record.json"
            if native
            else Path(commands[engine][0]).parent / "build-record.json"
        )
        path = retain(
            "build:" + engine,
            builds.get(engine, default),
            selected_record["sha256"]["imre_build_record" if native else "rust_record"],
        )
        value = json.loads(path.read_text())
        if native:
            prefix = "baseline_" if engine == "baseline" else ""
            retain(
                prefix + "compiler",
                Path(compile_command[0]),
                selected_record["sha256"]["compiler"],
            )
            retain(
                prefix + "runtime",
                Path(compile_command[compile_command.index("--runtime") + 1]),
                selected_record["sha256"]["runtime"],
            )
            retain(
                prefix + "llvm_ir",
                (args.llvm_ir if engine == "imre" else None)
                or Path(compile_command[compile_command.index("--emit-llvm") + 1]),
                selected_record["sha256"]["inventory_ir"],
            )
            source_hashes = value["source_sha256"]
            if not driver.contains_digest(value["sha256"], hashes[prefix + "compiler"]):
                raise ValueError("compiler build record identifies another compiler")
            if not driver.contains_digest(value["sha256"], hashes[prefix + "runtime"]):
                raise ValueError("compiler build record identifies another runtime")
        else:
            source_hashes = value["sources"]
            if (
                value["binaries"]["bigint"]["sha256"] != hashes["binary:" + engine]
                or source_hashes != record["rust_sources"]
            ):
                raise ValueError("Rust build record identifies another source/binary")
        if set(value["source_contents"]) != set(source_hashes) or any(
            validator.digest(value["source_contents"][name].encode()) != digest
            for name, digest in source_hashes.items()
        ):
            raise ValueError("frozen build source contents differ from their hashes")
    for name, digest in record["sources"].items():
        if name.startswith("inventory_reconciliation/"):
            retain("application:" + name, ROOT / "examples" / name, digest)
    for name in HELPERS:
        retain("harness:" + name, HERE / name)
    valgrind, profiler_files, environment = profiler(args)
    for name, path in profiler_files.items():
        retain("profiler:" + name, path)
    version = subprocess.check_output(
        [str(valgrind), "--version"], env=environment, text=True, timeout=args.timeout
    ).strip()

    output = args.output.absolute()
    if output.exists() or output.is_symlink():
        raise ValueError("profiling output must be a new directory")
    output.mkdir(parents=True, exist_ok=False)
    output = output.resolve()
    (output / "comparison.json").write_bytes(record_path.read_bytes())
    if "baseline_comparison" in inputs:
        (output / "baseline-comparison.json").write_bytes(
            inputs["baseline_comparison"].read_bytes()
        )
    source_dir = output / "sources"
    source_dir.mkdir()
    for name in HELPERS:
        (source_dir / name).write_bytes((HERE / name).read_bytes())
    build_dir = output / "build-records"
    build_dir.mkdir()
    for engine in engines:
        (build_dir / (engine + ".json")).write_bytes(
            inputs["build:" + engine].read_bytes()
        )
    save(output / "workloads.json", {"workloads": [row for row, _, _ in workloads]})
    metadata = {
        "schema_version": 1,
        "method": driver.METHOD,
        "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "comparison_git": record["git"],
        "comparison_options": record["options"],
        "comparison_tools": record["tools"],
        "comparison_was_validation_only": record["options"]["validation_only"],
        "inputs": {
            name: {
                "path": str(path),
                "sha256": hashes[name],
                "bytes": path.stat().st_size,
            }
            for name, path in inputs.items()
        },
        "source_scope": "Preserved build snapshots identify compiler/runtime/Rust sources; current application bytes are verified. No artifact is rebuilt.",
        "profiler": version,
        "cpu": cpu,
        "host": {
            "os": platform.system(),
            "architecture": platform.machine(),
            "kernel": platform.release(),
        },
        "tracer_pid_before": before,
        "perf_event_paranoid": Path("/proc/sys/kernel/perf_event_paranoid")
        .read_text()
        .strip(),
        "hardware_counters": "Not collected by this driver; host perf_event_paranoid is recorded separately. Callgrind counts instructions, not cycles or CPU-time percentages.",
        "environment": {
            name: environment.get(name)
            for name in (
                "VALGRIND_LIB",
                "VALGRIND_OPTS",
                "LANG",
                "LC_ALL",
                "LD_PRELOAD",
                "LD_LIBRARY_PATH",
                "MALLOC_ARENA_MAX",
                "MALLOC_PERTURB_",
                "GLIBC_TUNABLES",
            )
        },
        "environment_scope": "All engines inherit the same harness environment; VALGRIND_OPTS is cleared and VALGRIND_LIB selects the hashed profiler installation. This allowlist is not a complete environment snapshot.",
        "filesystem": "Fresh retained cwd per engine/tool; complete captured input and expected output bytes checked independently. Program file I/O is profiled, fixture creation and validation are outside instrumentation. No fsync or cold-cache claim.",
        "runs": [],
    }
    summary = {"schema_version": 1, "method": driver.METHOD, "runs": []}
    save(output / "provenance.json", metadata)

    def unchanged() -> None:
        for name, path in inputs.items():
            if shared.sha256(path) != hashes[name]:
                raise ValueError(f"input changed during profiling: {name}")

    for workload, case, expected in workloads:
        for engine, binary in binaries.items():
            for tool in args.tools:
                unchanged()
                label = f"{case.label}.{engine}.{tool}"
                directory = output / "files" / label
                directory.mkdir(parents=True)
                validator.materialize(case, directory)
                trace, log = output / (label + ".trace"), output / (label + ".log")
                command = [
                    str(valgrind),
                    "--command-line-only=yes",
                    f"--tool={tool}",
                    "--num-callers=24",
                    f"--log-file={log}",
                ]
                command += (
                    [
                        "--dump-instr=yes",
                        "--collect-jumps=yes",
                        f"--callgrind-out-file={trace}",
                    ]
                    if tool == "callgrind"
                    else ["--mode=heap", f"--dhat-out-file={trace}"]
                )
                command += [str(binary), *case.arguments]
                start = time.perf_counter_ns()
                timed_out = False
                try:
                    process = subprocess.run(
                        command,
                        input=b"",
                        capture_output=True,
                        check=False,
                        cwd=directory,
                        env=environment,
                        timeout=args.timeout,
                    )
                except subprocess.TimeoutExpired as error:
                    timed_out = True
                    process = subprocess.CompletedProcess(
                        command, -1, error.stdout or b"", error.stderr or b""
                    )
                elapsed = time.perf_counter_ns() - start
                for stream in ("stdout", "stderr"):
                    (output / (label + "." + stream)).write_bytes(
                        getattr(process, stream)
                    )
                item = {
                    "workload": case.label,
                    "engine": engine,
                    "tool": tool,
                    "command": command,
                    "cwd": str(directory.relative_to(output)),
                    "diagnostic_elapsed_ns": elapsed,
                    "timed_out": timed_out,
                    "returncode": process.returncode,
                    "stdout_sha256": validator.digest(process.stdout),
                    "stderr_sha256": validator.digest(process.stderr),
                    "trace": trace.name,
                    "trace_sha256": shared.sha256(trace) if trace.exists() else None,
                    "log": log.name,
                    "log_sha256": shared.sha256(log) if log.exists() else None,
                    "complete_output_verified": False,
                }
                metadata["runs"].append(item)
                save(output / "provenance.json", metadata)
                if timed_out or not trace.exists():
                    raise RuntimeError(
                        f"profile failed: {label}; partial evidence retained"
                    )
                item["result"] = validator.check_result(
                    case, expected, process, directory
                )
                item["complete_output_verified"] = True
                unchanged()
                values = (
                    driver.callgrind(trace)
                    if tool == "callgrind"
                    else driver.dhat(trace)
                )
                summary["runs"].append(
                    {
                        "workload": case.label,
                        "engine": engine,
                        "tool": tool,
                        "trace_sha256": item["trace_sha256"],
                        **values,
                    }
                )
                save(output / "summary.json", summary)
                save(output / "provenance.json", metadata)
                counts = (
                    f"{values['total_instructions']:,} instructions"
                    if tool == "callgrind"
                    else f"{values['totals']['tbk']:,} allocated blocks, {values['totals']['tb']:,} requested bytes, {values['totals']['gb']:,} peak live bytes"
                )
                print(f"Verified {label}: {counts}", flush=True)
    unchanged()
    metadata["tracer_pid_after"] = tracer_pid()
    if metadata["tracer_pid_after"]:
        raise RuntimeError("profiler harness became ptrace-traced")
    metadata["completed_at_utc"] = datetime.now(timezone.utc).isoformat()
    save(output / "provenance.json", metadata)
    save(
        output / "manifest.json",
        {
            path.relative_to(output).as_posix(): {
                "bytes": path.stat().st_size,
                "sha256": shared.sha256(path),
            }
            for path in sorted(output.rglob("*"))
            if path.is_file()
        },
    )
    print(f"Saved {output}", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--record", type=Path, required=True)
    parser.add_argument(
        "--baseline-record",
        type=Path,
        help="Original comparison required with --engine baseline",
    )
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--engine", dest="engines", choices=ENGINES, action="append")
    parser.add_argument("--workload", dest="workloads", action="append")
    parser.add_argument(
        "--binary",
        type=driver.named_path,
        action="append",
        default=[],
        metavar="NAME=PATH",
    )
    parser.add_argument(
        "--build-record",
        type=driver.named_path,
        action="append",
        default=[],
        metavar="NAME=JSON",
    )
    parser.add_argument(
        "--llvm-ir", type=Path, help="Relocated Imre IR; must retain its recorded hash"
    )
    parser.add_argument(
        "--tools",
        nargs="+",
        choices=("callgrind", "dhat"),
        default=["callgrind", "dhat"],
    )
    parser.add_argument("--valgrind")
    parser.add_argument("--valgrind-lib", type=Path)
    parser.add_argument("--cpu", type=int)
    parser.add_argument("--timeout", type=shared.positive_float, default=600)
    profile(parser.parse_args())


if __name__ == "__main__":
    main()
