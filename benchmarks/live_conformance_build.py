"""Build the developer-tool entrypoints from retained, verified inputs."""

from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import runpy
import shutil
import subprocess
import sys
import time
from types import SimpleNamespace


ROOT = Path(__file__).resolve().parents[1]
HERE = ROOT / "benchmarks"
benchmark = SimpleNamespace(**runpy.run_path(str(HERE / "live_conformance_benchmark.py")))
shared = benchmark.shared


def source_paths(application: str) -> list[Path]:
    paths = [Path(__file__), HERE / "live_conformance_benchmark.py",
             HERE / "live_conformance_validate.py", HERE / "native_tools_benchmark.py",
             HERE / "conformance_validate.py", HERE / "benchmark.py", HERE / "validate.py"]
    for directory in (ROOT / "examples" / application, ROOT / "stdlib/relar"):
        paths.extend(path for path in directory.rglob("*.py") if "__pycache__" not in path.parts)
    return sorted(set(paths))


def main() -> None:
    if sys.flags.optimize:
        raise SystemExit("build validation requires assertions")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--core-build-record", required=True, type=Path)
    parser.add_argument("--application", choices=("conformance_report", "python_catalog"), default="conformance_report")
    parser.add_argument("--mode", choices=("normal", "thinlto", "asan"), default="normal")
    parser.add_argument("--clang", default="clang-18")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--allow-dirty", action="store_true", help="exploratory builds only")
    args = parser.parse_args()
    if (args.mode == "thinlto") != (args.lto_linker is not None):
        parser.error("supply --lto-linker exactly for --mode thinlto")
    core_path = args.core_build_record.resolve(strict=True)
    guarded = {str(core_path): benchmark.identity(core_path)}
    core = benchmark.verify_build_record(core_path, guarded)
    assert core["kind"] == "native-tools-build"
    assert core["source_matches_revision"] or args.allow_dirty, "core inputs were exploratory"
    compiler = core_path.parent / "relar"
    runtime = core_path.parent / (args.mode + "_runtime.a")
    assert benchmark.digest(runtime) == core["sha256"][args.mode + "_runtime"]
    clang = shutil.which(args.clang)
    if clang is None:
        parser.error("Clang executable was not found")
    tools = {"clang": Path(clang).resolve(strict=True)}
    if args.lto_linker is not None:
        # ld.lld's spelling chooses the GNU linker driver even when it is a symlink.
        tools["lto_linker"] = args.lto_linker.absolute()
        assert tools["lto_linker"].is_file()
    sources = source_paths(args.application)
    guarded.update({str(path): benchmark.identity(path) for path in [*sources, *tools.values()]})
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    hashes = {str(path): benchmark.digest(path) for path in sources}
    matches = core["source_matches_revision"]
    for path in sources:
        committed = subprocess.run(["git", "show", revision + ":" + path.relative_to(ROOT).as_posix()],
                                   cwd=ROOT, capture_output=True, check=False)
        matches &= committed.returncode == 0 and hashlib.sha256(committed.stdout).hexdigest() == hashes[str(path)]
    if not matches and not args.allow_dirty:
        parser.error("program inputs differ from HEAD; --allow-dirty is exploratory only")
    output = args.output.resolve()
    if output.exists() or any(Path(path).is_relative_to(output) or output.is_relative_to(Path(path))
                              for path in guarded):
        parser.error("output must be a new directory separate from all build inputs")
    output.mkdir(parents=True, exist_ok=False)
    retained = {}
    for path in sources:
        destination = output / "sources" / path.relative_to(ROOT)
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(path, destination)
        retained[str(path.relative_to(ROOT))] = benchmark.identity(destination)
    environment = {"PATH": "/usr/bin:/bin", "HOME": os.environ.get("HOME", "/tmp"),
                   "LANG": "C.UTF-8", "LC_ALL": "C.UTF-8", "TZ": "UTC"}
    tool_versions = {name: subprocess.check_output([str(path), "--version"], env=environment,
                                                  text=True, timeout=30)
                     for name, path in tools.items()}
    kind = "live-conformance-program-build" if args.application == "conformance_report" else "streaming-catalog-program-build"
    record = {"schema": 1, "kind": kind, "status": "running",
              "mode": args.mode, "target": core["target"], "revision": revision,
              "source_matches_revision": matches, "input_guards_passed": False,
              "core_build_record": benchmark.identity(core_path),
              "compiler": benchmark.identity(compiler), "runtime": benchmark.identity(runtime),
              "tools": {name: benchmark.identity(path) for name, path in tools.items()},
              "tool_versions": tool_versions, "source_inputs": hashes, "retained_sources": retained,
              "environment": environment, "input_identities": guarded, "steps": [],
              "binaries": {}, "llvm": {}, "linked_libraries": {}}
    try:
        for suffix, entry in (("", "__main__.py"), ("-compute", "compute.py")):
            name = args.application + suffix
            binary = output / ("conformance-report" + suffix if args.application == "conformance_report" else name)
            llvm = output / (binary.name + ".ll")
            command = [str(compiler), "build", str(ROOT / "examples" / args.application / entry),
                       "--module-root", str(ROOT / "examples"), "-o", str(binary),
                       "--runtime", str(runtime), "--clang", str(tools["clang"]),
                       "--opt-level", "3", "--emit-llvm", str(llvm), "--target", core["target"]]
            if args.mode == "thinlto":
                command += ["--lto-linker", str(tools["lto_linker"])]
            if args.mode == "asan":
                command += ["--sanitize"]
            started = time.perf_counter_ns()
            completed = benchmark.live.run_process(command, directory=ROOT, env=environment, timeout=1200)
            step = {"name": name, "command": command, "cwd": str(ROOT),
                    "returncode": completed.returncode, "elapsed_ns": time.perf_counter_ns() - started}
            for stream in ("stdout", "stderr"):
                path = output / (binary.name + "." + stream)
                path.write_bytes(getattr(completed, stream))
                step[stream] = benchmark.identity(path)
            record["steps"].append(step)
            assert completed.returncode == 0, (name, completed.stderr.decode(errors="replace"))
            assert shared.elf_target(binary) == shared.elf_target(compiler), "program target differs"
            record["binaries"][name] = benchmark.identity(binary)
            record["llvm"][name] = benchmark.identity(llvm)
            record["linked_libraries"][str(binary)] = shared.linked_libraries(binary)
            print("Built " + str(binary), flush=True)
        assert source_paths(args.application) == sources, "program source membership changed during build"
        assert all(benchmark.identity(Path(path)) == item for path, item in guarded.items()), "build input changed"
        assert all(benchmark.identity(Path(item["path"])) == item for item in retained.values()), "retained source changed"
        assert revision == subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        record.update(status="passed", input_guards_passed=True)
    except BaseException as error:
        record.update(status="failed", error=str(error))
        raise
    finally:
        record["completed_at_utc"] = datetime.now(timezone.utc).isoformat()
        (output / "build-record.json").write_text(json.dumps(record, indent=2) + "\n")
    print(str(output / "build-record.json"))


if __name__ == "__main__":
    main()
