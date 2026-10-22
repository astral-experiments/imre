"""Profile frozen native tools against complete expected files with Callgrind/DHAT."""

import argparse
import hashlib
import json
import os
import platform
import runpy
import shutil
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace

if sys.flags.optimize:
    raise SystemExit("profile validation requires assertions; do not use -O")

HERE = Path(__file__).resolve().parent
driver = SimpleNamespace(**runpy.run_path(str(HERE / "dependency_profile.py")))


def digest(data):
    return hashlib.sha256(data).hexdigest()


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def named(value):
    name, separator, source = value.partition("=")
    relative = Path(name)
    if not separator or not source or not relative.parts or relative.is_absolute() or ".." in relative.parts:
        raise argparse.ArgumentTypeError("expected a relative NAME=SOURCE file mapping")
    return name, Path(source).resolve(strict=True)


def unique(values):
    result = dict(values)
    if len(result) != len(values):
        raise ValueError("file mappings must have unique names")
    return result


def tree_inventory(directory):
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError(f"expected a regular directory: {directory}")
    files, directories = {}, []
    for path in sorted(directory.rglob("*")):
        name = str(path.relative_to(directory))
        if path.is_symlink():
            raise ValueError(f"unexpected symlink: {path}")
        if path.is_dir():
            directories.append(name)
        elif path.is_file():
            files[name] = {"bytes": path.stat().st_size, "sha256": digest(path.read_bytes())}
        else:
            raise ValueError(f"unexpected filesystem entry: {path}")
    return {"files": files, "directories": directories}


def inventory(directory):
    return tree_inventory(directory)["files"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=named, action="append", required=True)
    parser.add_argument("--input", type=named, action="append", default=[])
    parser.add_argument("--expected", type=named, action="append", default=[])
    parser.add_argument("--stdout", default="")
    parser.add_argument("--guard-dir", type=Path, action="append", default=[])
    parser.add_argument("--retain", type=Path, action="append", default=[])
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--tools", nargs="+", choices=("callgrind", "dhat"), default=["callgrind", "dhat"])
    parser.add_argument("--timeout", type=float, default=600)
    parser.add_argument("--cpu", type=int)
    parser.add_argument("arguments", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    binaries, inputs, expected = map(unique, (args.binary, args.input, args.expected))
    if not binaries or len(set(args.tools)) != len(args.tools) or args.timeout <= 0:
        parser.error("select distinct profilers and a positive timeout")
    if any(Path(name).name != name for name in binaries):
        parser.error("engine names must be single path components")
    arguments = args.arguments[1:] if args.arguments[:1] == ["--"] else args.arguments
    output = args.output.absolute()
    protected = [*binaries.values(), *inputs.values(), *expected.values(),
                 *(path.resolve(strict=True) for path in args.retain)]
    if any(directory.is_symlink() for directory in args.guard_dir):
        parser.error("guarded source directories must not be symlinks")
    guard_trees = {str(directory.resolve(strict=True)): tree_inventory(directory.resolve(strict=True))
                   for directory in args.guard_dir}
    guards = {str(Path(directory) / name): identity["sha256"]
              for directory, tree in guard_trees.items() for name, identity in tree["files"].items()}
    if output.exists() or output.is_symlink() or any(path.is_relative_to(output.resolve()) for path in protected):
        parser.error("output must be a new directory separate from every input")
    for directory in args.guard_dir:
        directory = directory.resolve(strict=True)
        if output.resolve().is_relative_to(directory) or directory.is_relative_to(output.resolve()):
            parser.error("output must not overlap a guarded source tree")
    affinity = os.sched_getaffinity(0)
    cpu = min(affinity) if args.cpu is None else args.cpu
    if cpu not in affinity:
        parser.error("CPU is outside the allowed affinity")
    tracer = int(next(line.split(":", 1)[1] for line in Path("/proc/self/status").read_text().splitlines()
                      if line.startswith("TracerPid:")))
    if tracer:
        parser.error("run the profile outside a ptrace sandbox")
    os.sched_setaffinity(0, {cpu})
    executable = shutil.which("valgrind")
    if executable is None:
        parser.error("Valgrind is not installed")
    valgrind = Path(executable).resolve(strict=True)
    library = next((path for path in (Path("/usr/libexec/valgrind"), Path("/usr/lib/valgrind"),
                                      Path("/usr/lib/x86_64-linux-gnu/valgrind"))
                    if (path / "callgrind-amd64-linux").is_file()), None)
    if library is None:
        parser.error("cannot locate the installed native Valgrind tools")
    environment = {**os.environ, "VALGRIND_LIB": str(library), "VALGRIND_OPTS": ""}
    version = subprocess.check_output([str(valgrind), "--version"], env=environment, text=True).strip()
    output.mkdir(parents=True, exist_ok=False)
    for directory in ("bin", "inputs", "expected", "retained", "files"):
        (output / directory).mkdir()
    copied = {}
    for group, paths in (("bin", binaries), ("inputs", inputs), ("expected", expected)):
        for name, source in paths.items():
            destination = output / group / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination)
            copied[f"{group}/{name}"] = {"source": str(source), "sha256": digest(destination.read_bytes())}
    for index, source in enumerate([Path(__file__), HERE / "dependency_profile.py", *args.retain]):
        source = source.resolve(strict=True)
        destination = output / "retained" / f"{index:02d}-{source.name}"
        shutil.copy2(source, destination)
        copied[str(destination.relative_to(output))] = {"source": str(source), "sha256": digest(destination.read_bytes())}
    profiler_paths = [valgrind, *(library / (tool + "-amd64-linux") for tool in args.tools),
                      *sorted(library.glob("vgpreload_*-amd64-linux.so"))]
    profiler_hashes = {str(path): digest(path.read_bytes()) for path in profiler_paths}
    expected_files = {**inventory(output / "inputs"), **inventory(output / "expected")}
    expected_directories = {str(parent) for name in expected_files for parent in Path(name).parents if str(parent) != "."}
    record = {"schema": 1, "kind": "native-tools-profile", "status": "running", "method": driver.METHOD,
              "started_at_utc": datetime.now(timezone.utc).isoformat(), "cpu": cpu,
              "host": {"architecture": platform.machine(), "kernel": platform.release()},
              "profiler_version": version, "profiler_sha256": profiler_hashes,
              "perf_event_paranoid": Path("/proc/sys/kernel/perf_event_paranoid").read_text().strip(),
              "arguments": arguments, "copied_inputs": copied, "guarded_sources": guards,
              "guarded_source_trees": guard_trees,
              "expected_files": expected_files, "expected_stdout": args.stdout, "runs": [],
              "input_guards_passed": False}
    save(output / "provenance.json", record)
    summary = {"schema": 1, "kind": "native-tools-profile-summary", "method": driver.METHOD, "runs": []}

    def unchanged():
        assert all(digest((output / name).read_bytes()) == value["sha256"] for name, value in copied.items())
        assert {name: tree_inventory(Path(name)) for name in guard_trees} == guard_trees, "guarded source tree changed"
        assert all(digest(Path(name).read_bytes()) == value for name, value in profiler_hashes.items())
        assert os.sched_getaffinity(0) == {cpu}

    try:
        for engine in binaries:
            for tool in args.tools:
                unchanged()
                label = engine + "." + tool
                directory = output / "files" / label
                shutil.copytree(output / "inputs", directory)
                trace, log = output / (label + ".trace"), output / (label + ".log")
                command = [str(valgrind), "--command-line-only=yes", "--tool=" + tool,
                           "--num-callers=24", "--log-file=" + str(log)]
                command += (["--dump-instr=yes", "--collect-jumps=yes", "--callgrind-out-file=" + str(trace)]
                            if tool == "callgrind" else ["--mode=heap", "--dhat-out-file=" + str(trace)])
                command += [str(output / "bin" / engine), *arguments]
                start = time.perf_counter_ns()
                process = subprocess.run(command, input=b"", capture_output=True, cwd=directory,
                                         env=environment, timeout=args.timeout, check=False)
                elapsed = time.perf_counter_ns() - start
                for stream in ("stdout", "stderr"):
                    (output / (label + "." + stream)).write_bytes(getattr(process, stream))
                assert process.returncode == 0 and not process.stderr and process.stdout == args.stdout.encode(), (label, process.returncode, process.stderr)
                actual = tree_inventory(directory)
                assert actual["files"] == expected_files, (label, "complete output files differ")
                assert set(actual["directories"]) == expected_directories, (label, "output directories differ")
                unchanged()
                parsed = driver.callgrind(trace) if tool == "callgrind" else driver.dhat(trace)
                item = {"engine": engine, "tool": tool, "command": command,
                        "diagnostic_elapsed_ns": elapsed, "returncode": process.returncode,
                        "trace_sha256": digest(trace.read_bytes()), "log_sha256": digest(log.read_bytes()),
                        "complete_outputs_verified": True}
                record["runs"].append(item)
                summary["runs"].append({**item, **parsed})
                save(output / "summary.json", summary)
                save(output / "provenance.json", record)
                counts = {"instructions": parsed["total_instructions"]} if tool == "callgrind" else parsed["totals"]
                print(json.dumps({"engine": engine, "tool": tool, **counts}), flush=True)
        unchanged()
        record.update(status="passed", input_guards_passed=True)
    except BaseException as error:
        record.update(status="failed", error=repr(error))
        raise
    finally:
        record["completed_at_utc"] = datetime.now(timezone.utc).isoformat()
        save(output / "provenance.json", record)


if __name__ == "__main__":
    main()
