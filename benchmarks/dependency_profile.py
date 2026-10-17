"""Profile preserved planner binaries using captured inputs and complete outputs.

Callgrind counts executed instructions; DHAT records allocation traffic and
lifetimes. Instrumented elapsed times are diagnostic, never benchmark results.
"""

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import time
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
METHOD = (
    "Callgrind Ir counts executed instructions, not hardware CPU cycles or "
    "elapsed-time shares. Exclusive function counts reconcile with the profiler "
    "total; inclusive edge costs overlap. DHAT replaces allocator routines and "
    "records requested heap bytes, blocks, and lifetimes by allocation stack; "
    "these bytes are allocation traffic, not RSS. Instrumented elapsed times "
    "are excluded from performance comparisons."
)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def file_hash(path: Path) -> str:
    return digest(path.read_bytes())


def save_json(path: Path, value: dict) -> None:
    path.write_text(json.dumps(value, indent=2) + "\n")


def named_path(value: str) -> tuple[str, Path]:
    name, separator, path = value.partition("=")
    if not separator or not re.fullmatch(r"[A-Za-z0-9_-]+", name) or not path:
        raise argparse.ArgumentTypeError("expected NAME=PATH, with a simple name")
    return name, Path(path).expanduser().resolve()


def unique_paths(items: list[tuple[str, Path]], option: str) -> dict[str, Path]:
    result = dict(items)
    if len(result) != len(items):
        raise ValueError(f"{option} names must be unique")
    return result


def contains_digest(value: object, expected: str) -> bool:
    if isinstance(value, dict):
        return any(contains_digest(item, expected) for item in value.values())
    if isinstance(value, list):
        return any(contains_digest(item, expected) for item in value)
    return value == expected


def callgrind(path: Path) -> dict:
    lines = path.read_text().splitlines()
    assert "positions: instr line" in lines and "events: Ir" in lines
    names = {}
    objects = {}
    for line in lines:
        match = re.match(r"(?:c?fn)=\((\d+)\) (.+)", line)
        if match:
            names[match[1]] = match[2]
        match = re.match(r"(?:c?ob)=\((\d+)\) (.+)", line)
        if match:
            objects[match[1]] = match[2]
    exclusive, calls, inclusive = Counter(), Counter(), Counter()
    edge_calls, edge_costs = Counter(), Counter()
    function = callee = None
    current_object = None
    function_objects = {}
    call_cost = False
    for line in lines:
        match = re.match(r"ob=\((\d+)\)", line)
        if match:
            current_object = objects[match[1]]
            continue
        match = re.match(r"fn=\((\d+)\)", line)
        if match:
            function = match[1]
            function_objects.setdefault(names[function], set()).add(current_object)
            continue
        match = re.match(r"cfn=\((\d+)\)", line)
        if match:
            callee = match[1]
            continue
        if line.startswith("calls="):
            count = int(line.split()[0].split("=")[1])
            calls[names[callee]] += count
            edge_calls[names[function], names[callee]] += count
            call_cost = True
            continue
        if re.match(r"^(?:0x[0-9a-f]+|[+\-]\d+|\*|\d+) ", line):
            columns = line.split()
            if len(columns) != 3:
                continue
            count = int(columns[2])
            if call_cost:
                inclusive[names[callee]] += count
                edge_costs[names[function], names[callee]] += count
                call_cost = False
            else:
                exclusive[names[function]] += count
    total = int(next(line.split()[1] for line in lines if line.startswith("summary:")))
    assert sum(exclusive.values()) == total
    return {
        "total_instructions": total,
        "exclusive_sum_verified": True,
        "functions": [
            {
                "name": name,
                "self_instructions": count,
                "self_fraction": count / total,
                "objects": sorted(
                    value
                    for value in function_objects.get(name, ())
                    if value is not None
                ),
                "calls": calls[name],
                "incoming_inclusive_instructions": inclusive[name],
            }
            for name, count in exclusive.most_common()
        ],
        "edges": [
            {
                "caller": caller,
                "callee": callee,
                "calls": count,
                "inclusive_instructions": edge_costs[caller, callee],
            }
            for (caller, callee), count in edge_calls.most_common()
        ],
    }


def dhat(path: Path) -> dict:
    data = json.loads(path.read_text())
    totals = {
        key: sum(point[key] for point in data["pps"])
        for key in ("tb", "tbk", "gb", "gbk", "eb", "ebk", "rb", "wb")
    }
    points = []
    by_runtime_entry = {}
    for index, point in enumerate(data["pps"]):
        stack = [data["ftbl"][frame] for frame in point["fs"]]
        item = {key: value for key, value in point.items() if key not in ("fs", "acc")}
        item.update(
            {
                "id": index,
                "stack": stack,
                "mean_lifetime_in_instructions": point["tl"] / point["tbk"],
            }
        )
        points.append(item)
        entry = "other"
        for frame in stack:
            match = re.search(r": (relar_[a-z_]+) ", frame)
            if match:
                entry = match[1]
                break
        counter = by_runtime_entry.setdefault(entry, Counter())
        for key in totals:
            counter[key] += point[key]
    return {
        "schema": data["dhatFileVersion"],
        "mode": data["mode"],
        "lifetime_unit": data["tu"],
        "end_time": data["te"],
        "global_peak_time": data["tg"],
        "totals": totals,
        "by_nearest_runtime_entry": sorted(
            [
                {"entry": entry, **counter}
                for entry, counter in by_runtime_entry.items()
            ],
            key=lambda item: item["tb"],
            reverse=True,
        ),
        "points": sorted(points, key=lambda point: point["tb"], reverse=True),
    }


def summarize(directory: Path, output: Path) -> None:
    """Keep every function, edge, and allocation stack in the machine-readable result."""
    result = {
        "schema_version": 1,
        "method": METHOD,
        "harness_sha256": file_hash(Path(__file__)),
        "runs": [],
    }
    traces = sorted(directory.glob("*.trace"))
    if not traces:
        raise ValueError(f"no profiler traces in {directory}")
    for path in traces:
        workload, engine, tool, _ = path.name.split(".")
        if tool not in ("callgrind", "dhat"):
            raise ValueError(f"unknown trace type: {path}")
        values = callgrind(path) if tool == "callgrind" else dhat(path)
        result["runs"].append(
            {
                "workload": workload,
                "engine": engine,
                "tool": tool,
                "trace_sha256": file_hash(path),
                **values,
            }
        )
        if tool == "callgrind":
            print(f"{workload} {engine}: {values['total_instructions']:,} instructions")
        else:
            totals = values["totals"]
            print(
                f"{workload} {engine}: {totals['tbk']:,} allocated blocks, "
                f"{totals['tb']:,} requested bytes, {totals['gb']:,} peak live bytes"
            )
    save_json(output, result)


def profile(args: argparse.Namespace) -> None:
    if not args.record or not args.binary or not args.output:
        raise ValueError("profiling requires --record, --binary, and --output")
    if args.timeout <= 0 or len(set(args.tools)) != len(args.tools):
        raise ValueError("timeout must be positive and profiler names must be unique")
    binaries = unique_paths(args.binary, "--binary")
    build_paths = unique_paths(args.build_record, "--build-record")
    if build_paths.keys() - binaries.keys():
        raise ValueError("each --build-record name must match a --binary name")
    record_bytes = args.record.read_bytes()
    record = json.loads(record_bytes)
    hashes = {name: file_hash(path) for name, path in binaries.items()}
    builds = {}
    for name, path in build_paths.items():
        value = json.loads(path.read_bytes())
        if not contains_digest(value, hashes[name]):
            raise ValueError(f"build record does not contain the {name} binary hash")
        builds[name] = {"path": str(path), "sha256": file_hash(path), "record": value}
    captured = {(case["shape"], case["tasks"]): case for case in record["workloads"]}
    sizes = args.sizes or sorted({size for _, size in captured})
    shapes = args.shapes or sorted({shape for shape, _ in captured})
    selected = [(shape, size) for shape in shapes for size in sizes]
    if not selected or len(set(selected)) != len(selected):
        raise ValueError("select at least one workload, without duplicates")
    if not set(selected) <= captured.keys():
        raise ValueError("selected workload is absent from the input record")
    for shape, size in selected:
        if not re.fullmatch(r"[A-Za-z0-9_-]+", shape) or size <= 0:
            raise ValueError("invalid captured workload label")
        case = captured[shape, size]
        expected = case["expected_stdout"].encode()
        if (
            digest(case["input"].encode()) != case["input_sha256"]
            or digest(expected) != case["expected_stdout_sha256"]
            or len(expected) != case["expected_stdout_bytes"]
        ):
            raise ValueError(f"captured input/output evidence differs: {shape}-{size}")
    valgrind = Path(
        args.valgrind
        or shutil.which("valgrind")
        or Path.home() / ".cache/relar/profiling-tools/root/usr/bin/valgrind"
    )
    valgrind = Path(shutil.which(str(valgrind)) or valgrind).resolve()
    library = args.valgrind_lib or os.environ.get("VALGRIND_LIB")
    if library is None:
        for relative in ("libexec/valgrind", "lib/valgrind"):
            candidate = valgrind.parent.parent / relative
            if candidate.is_dir():
                library = candidate
                break
    environment = {**os.environ, "VALGRIND_OPTS": ""}
    profiler_files = {"valgrind": valgrind}
    if library:
        library = Path(library).resolve()
        environment["VALGRIND_LIB"] = str(library)
        arch = {"x86_64": "amd64", "aarch64": "arm64"}.get(
            platform.machine(), platform.machine()
        )
        profiler_files.update(
            {tool: library / f"{tool}-{arch}-linux" for tool in args.tools}
        )
    version = subprocess.check_output(
        [str(valgrind), "--version"], env=environment, text=True, timeout=args.timeout
    ).strip()
    affinity = os.sched_getaffinity(0)
    cpu = args.cpu if args.cpu is not None else min(affinity)
    if cpu not in affinity:
        raise ValueError("CPU must be in the process's allowed affinity")
    os.sched_setaffinity(0, {cpu})
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    (args.output / "input-record.json").write_bytes(record_bytes)
    (args.output / "harness.py").write_bytes(Path(__file__).read_bytes())
    provenance = {
        "schema_version": 1,
        "method": METHOD,
        "started_at_utc": datetime.now(timezone.utc).isoformat(),
        "input_record": {
            "path": str(args.record.resolve()),
            "sha256": digest(record_bytes),
        },
        "input_record_git": record.get("git"),
        "input_record_sources": record.get("sources"),
        "binaries": {
            name: {
                "path": str(path),
                "sha256": hashes[name],
                "bytes": path.stat().st_size,
                "hash_present_in_input_record": contains_digest(record, hashes[name]),
            }
            for name, path in binaries.items()
        },
        "build_records": builds,
        "profiler": version,
        "profiler_files": {
            name: {"path": str(path), "sha256": file_hash(path)}
            for name, path in profiler_files.items()
        },
        "profiler_library": environment.get("VALGRIND_LIB"),
        "harness_sha256": file_hash(Path(__file__)),
        "affinity": [cpu],
        "host": {
            "os": platform.system(),
            "architecture": platform.machine(),
            "kernel": platform.release(),
        },
        "io": "Captured input and full stdout use pipes; every stdout is saved and checked byte-for-byte.",
        "runs": [],
    }
    save_json(args.output / "provenance.json", provenance)
    for shape, size in selected:
        case, label = captured[shape, size], f"{shape}-{size}"
        data, expected = case["input"].encode(), case["expected_stdout"].encode()
        (args.output / f"{label}.stdin").write_bytes(data)
        (args.output / f"{label}.expected.stdout").write_bytes(expected)
        for name, binary in binaries.items():
            for tool in args.tools:
                if file_hash(binary) != hashes[name]:
                    raise ValueError(f"binary changed during profiling: {name}")
                stem = args.output / f"{label}.{name}.{tool}"
                trace, log = Path(f"{stem}.trace"), Path(f"{stem}.log")
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
                command.append(str(binary))
                start = time.perf_counter_ns()
                timed_out = False
                try:
                    process = subprocess.run(
                        command,
                        input=data,
                        capture_output=True,
                        check=False,
                        cwd=ROOT,
                        env=environment,
                        timeout=args.timeout,
                    )
                    stdout, stderr, returncode = (
                        process.stdout,
                        process.stderr,
                        process.returncode,
                    )
                except subprocess.TimeoutExpired as error:
                    stdout, stderr, returncode = (
                        error.stdout or b"",
                        error.stderr or b"",
                        None,
                    )
                    timed_out = True
                elapsed = time.perf_counter_ns() - start
                Path(f"{stem}.stdout").write_bytes(stdout)
                Path(f"{stem}.stderr").write_bytes(stderr)
                item = {
                    "shape": shape,
                    "tasks": size,
                    "engine": name,
                    "tool": tool,
                    "command": command,
                    "diagnostic_elapsed_ns": elapsed,
                    "input_sha256": case["input_sha256"],
                    "expected_stdout_sha256": case["expected_stdout_sha256"],
                    "stdout_sha256": digest(stdout),
                    "stdout_bytes": len(stdout),
                    "returncode": returncode,
                    "timed_out": timed_out,
                    "stderr_sha256": digest(stderr),
                    "stderr_bytes": len(stderr),
                    "output_matches": stdout == expected,
                    "trace": trace.name,
                    "trace_sha256": file_hash(trace) if trace.exists() else None,
                    "log": log.name,
                    "log_sha256": file_hash(log) if log.exists() else None,
                }
                provenance["runs"].append(item)
                provenance["updated_at_utc"] = datetime.now(timezone.utc).isoformat()
                save_json(args.output / "provenance.json", provenance)
                if (
                    returncode != 0
                    or stderr
                    or stdout != expected
                    or not trace.exists()
                ):
                    raise RuntimeError(
                        f"profile failed: {label}/{name}/{tool}; evidence saved in {args.output}"
                    )
                print(f"Verified {label}/{name}/{tool}", flush=True)
    for name, binary in binaries.items():
        if file_hash(binary) != hashes[name]:
            raise ValueError(f"binary changed during profiling: {name}")
    for name, path in profiler_files.items():
        if file_hash(path) != provenance["profiler_files"][name]["sha256"]:
            raise ValueError(f"profiler changed during profiling: {name}")
    if file_hash(args.record) != digest(record_bytes):
        raise ValueError("input record changed during profiling")
    provenance["completed_at_utc"] = datetime.now(timezone.utc).isoformat()
    save_json(args.output / "provenance.json", provenance)
    summarize(args.output, args.output / "summary.json")
    save_json(
        args.output / "manifest.json",
        {
            path.name: {"bytes": path.stat().st_size, "sha256": file_hash(path)}
            for path in sorted(args.output.iterdir())
            if path.is_file()
        },
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--record", type=Path, help="Captured dependency benchmark/comparison JSON"
    )
    parser.add_argument(
        "--binary", type=named_path, action="append", default=[], metavar="NAME=PATH"
    )
    parser.add_argument(
        "--build-record",
        type=named_path,
        action="append",
        default=[],
        metavar="NAME=JSON",
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="New evidence directory; existing directories are rejected",
    )
    parser.add_argument(
        "--sizes", type=int, nargs="+", help="Default: every captured size"
    )
    parser.add_argument("--shapes", nargs="+", help="Default: every captured shape")
    parser.add_argument(
        "--tools",
        nargs="+",
        choices=("callgrind", "dhat"),
        default=["callgrind", "dhat"],
    )
    parser.add_argument(
        "--valgrind",
        help="Executable or command name; default: PATH, then the Relar tool cache",
    )
    parser.add_argument(
        "--valgrind-lib",
        type=Path,
        help="Valgrind tool/data directory, such as usr/libexec/valgrind",
    )
    parser.add_argument("--cpu", type=int, help="Default: first allowed CPU")
    parser.add_argument("--timeout", type=float, default=300)
    parser.add_argument(
        "--summarize",
        type=Path,
        help="Summarize existing traces without executing binaries",
    )
    parser.add_argument(
        "--summary", type=Path, help="Summary output override for --summarize"
    )
    args = parser.parse_args()
    if args.summarize:
        summarize(args.summarize, args.summary or args.summarize / "summary.json")
    else:
        profile(args)


if __name__ == "__main__":
    main()
