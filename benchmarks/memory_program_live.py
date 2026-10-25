"""Time Ruff's live run command against the four already-prepared primer projects."""

import argparse
import gzip
import hashlib
import json
import os
import platform
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from conformance_regex_benchmark import file_id
from memory_validate import equivalent_original, expected_projects


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("source", "imre", "projects-dir", "old-ty", "new-ty", "output"):
        parser.add_argument(f"--{name}", type=Path, required=True)
    parser.add_argument("--python", type=Path, default=Path(sys.executable))
    parser.add_argument("--samples", type=int, default=4)
    args = parser.parse_args()
    if sys.flags.optimize or args.samples < 2 or args.samples % 2:
        parser.error("validation requires assertions and a positive even sample count")
    python = str(args.python.absolute())
    projects = sorted(
        p for p in args.projects_dir.resolve().iterdir() if (p / ".venv").is_dir()
    )
    if [p.name for p in projects] != ["flake8", "prefect", "sphinx", "trio"]:
        parser.error("supply the four checkouts created by Ruff's run command")
    source = args.source.resolve(strict=True)
    binaries = [str(tool.resolve(strict=True)) for tool in (args.old_ty, args.new_ty)]
    commands = {
        "cpython": [python, str(source)],
        "imre": [str(args.imre.resolve(strict=True))],
    }
    env = {**os.environ, "IMRE_PYTHON": python, "PYTHONHASHSEED": "0", "NO_COLOR": "1"}
    results = []
    for sample in range(args.samples + 1):
        # Alternate first position. Each execution produces its own eight fresh captures.
        order = tuple(commands) if sample % 2 else tuple(reversed(commands))
        for engine in order:
            with tempfile.TemporaryDirectory(prefix="imre-memory-live-") as tmp:
                root = Path(tmp)
                flags = [
                    "--old-ty",
                    binaries[0],
                    "--new-ty",
                    binaries[1],
                    "--projects-dir",
                    str(args.projects_dir.resolve()),
                    "--old-reports-dir",
                    "old",
                    "--new-reports-dir",
                    "new",
                    "--output",
                    "report.md",
                ]
                start = time.perf_counter_ns()
                live = subprocess.run(
                    [*commands[engine], "run", *flags],
                    cwd=root,
                    env=env,
                    capture_output=True,
                    check=False,
                )
                duration = time.perf_counter_ns() - start
                if live.returncode:
                    raise RuntimeError(live.stderr.decode())
                report = (root / "report.md").read_bytes()
                progress, marker, echoed = live.stderr.partition(
                    b"Output written to report.md\n"
                )
                assert not live.stdout and marker and echoed == report + b"\n"
                assert (
                    progress.count(b"already exists at ") == 4
                    and progress.count(b"Running ") == 8
                )
                paths = sorted(root.glob("*/*.json"))
                assert {p.relative_to(root).as_posix() for p in paths} == {
                    f"{side}/{p.name}.json" for side in ("old", "new") for p in projects
                }
                # ty's own allocator can change its totals between executions.
                oracle = subprocess.run(
                    [
                        *commands["cpython"],
                        "compare",
                        "--old-dir",
                        "old",
                        "--new-dir",
                        "new",
                    ],
                    cwd=root,
                    env=env,
                    capture_output=True,
                    check=True,
                )
                assert not oracle.stderr and oracle.stdout.endswith(b"\n")
                exact = oracle.stdout == report + b"\n"
                if not exact:
                    values = [
                        (
                            p.name,
                            *(
                                json.loads(
                                    (root / side / f"{p.name}.json").read_bytes()
                                )
                                for side in ("old", "new")
                            ),
                        )
                        for p in projects
                    ]
                    canonical, groups = expected_projects(values)
                    for output in (report, oracle.stdout[:-1]):
                        equivalent_original(
                            output.decode(),
                            {"rendered": canonical, "row_groups": groups},
                        )
                if sample:
                    results.append(
                        {
                            "sample": sample,
                            "engine": engine,
                            "ns": duration,
                            "report_bytes": len(report),
                            "report_sha256": hashlib.sha256(report).hexdigest(),
                            "matches_cpython_bytes": exact,
                            "captures": {
                                str(p.relative_to(root)): file_id(p)["sha256"]
                                for p in paths
                            },
                        }
                    )
    binaries_used = [
        source,
        source.with_name("setup_primer_project.py"),
        source.with_name("setup_primer_project.py.lock"),
        Path(commands["imre"][0]),
        Path(python),
        *(Path(b) for b in binaries),
        Path(shutil.which("uv")),
    ]
    result = {
        "platform": platform.platform(),
        "cpu_affinity": sorted(os.sched_getaffinity(0))
        if hasattr(os, "sched_getaffinity")
        else None,
        "median_ms": {
            engine: statistics.median(r["ns"] for r in results if r["engine"] == engine)
            / 1e6
            for engine in commands
        },
        "projects": {
            p.name: subprocess.check_output(
                ["git", "-C", p, "rev-parse", "HEAD"], text=True
            ).strip()
            for p in projects
        },
        "tools": [file_id(path) for path in binaries_used],
        "results": results,
        "versions": [
            subprocess.check_output([b, "--version"], text=True).strip()
            for b in [python, *binaries, "uv"]
        ],
    }
    args.output.write_bytes(
        gzip.compress(json.dumps(result, sort_keys=True).encode(), mtime=0)
    )
    print(result["median_ms"])


if __name__ == "__main__":
    main()
