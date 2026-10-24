"""Time the complete Ruff compare command on captured ty memory reports."""

import argparse
import gzip
import hashlib
import json
import os
import platform
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from conformance_regex_benchmark import file_id, orders
from conformance_validate import environment
from memory_benchmark import scaled
from memory_validate import PACKAGE, UPSTREAM, equivalent_original, expected_projects


def prepare(directory, project_copies, row_copies, changed):
    """Scale the number of projects or distinct rows within the two real projects."""
    old, new = directory / "old", directory / "new"
    old.mkdir(parents=True)
    new.mkdir()
    sample = PACKAGE / "sample"
    records = json.loads((sample / "provenance.json").read_bytes())["reports"]
    captures = {r["fixture"]: r["sha256"] for r in records}
    projects, byte_count, item_count = [], 0, 0
    input_hashes = {}
    for stem in ("inventory", "ruff-scripts"):
        names = [f"{stem}-old.json", f"{stem}.json" if changed else f"{stem}-old.json"]
        inputs = [(sample / name).read_bytes() for name in names]
        assert all(
            hashlib.sha256(raw).hexdigest() == captures[name]
            for raw, name in zip(inputs, names, strict=True)
        )
        before, after = map(json.loads, inputs)
        if row_copies != 1:
            before, after = scaled(before, row_copies), scaled(after, row_copies)
            inputs = [
                (json.dumps(report, indent=2) + "\n").encode()
                for report in (before, after)
            ]
        item_count += project_copies * sum(
            len(x[group]) for x in (before, after) for group in ("structs", "queries")
        )
        byte_count += project_copies * sum(map(len, inputs))
        input_hashes[stem] = [hashlib.sha256(raw).hexdigest() for raw in inputs]
        for copy in range(project_copies):
            project = f"{stem}-{copy}" if copy else stem
            (old / f"{project}.json").write_bytes(inputs[0])
            (new / f"{project}.json").write_bytes(inputs[1])
            projects.append((project, before, after))
    rendered, groups = expected_projects(sorted(projects, key=lambda p: p[0]))
    return {"rendered": rendered, "row_groups": groups}, {
        "projects": len(projects),
        "input_bytes": byte_count,
        "items_both_sides": item_count,
        "capture_sha256": captures,
        "old_new_input_sha256": input_hashes,
    }


def measure(commands, directory, oracle, samples):
    args = ["compare", "--old-dir", "old", "--new-dir", "new"]
    env = environment(directory)

    def invoke(name):
        return subprocess.run(
            [*commands[name], *args],
            cwd=directory,
            env=env,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=60,
            check=True,
        )

    original = invoke("cpython")
    assert not original.stderr and original.stdout.endswith(b"\n")
    equivalent_original(original.stdout[:-1].decode(), oracle)
    oracle_sha = hashlib.sha256(original.stdout).hexdigest()
    plan = orders(tuple(commands))
    rows, observed = [], {key: {} for key in commands}
    for index in range(samples + 1):
        row = {}
        for name in plan[(index - 1) % len(plan)]:
            start = time.perf_counter_ns()
            process = invoke(name)
            elapsed = time.perf_counter_ns() - start
            assert not process.stderr and process.stdout.endswith(b"\n"), (
                name,
                directory,
                process.stderr,
            )
            sha = hashlib.sha256(process.stdout).hexdigest()
            if sha not in observed[name] and process.stdout != original.stdout:
                # Ruff iterates a set of names before its stable sort. Only an exchange of
                # equal-magnitude rows (including the 15th-row cutoff) is permitted.
                equivalent_original(process.stdout[:-1].decode(), oracle)
            observed[name][sha] = observed[name].get(sha, 0) + int(index > 0)
            row[name] = elapsed
        if index:
            rows.append(row)
    medians = {
        name: statistics.median(row[name] for row in rows) / 1e6 for name in commands
    }
    return {
        "argv": args,
        "order": plan,
        "samples_ns": rows,
        "median_ms": medians,
        "imre_over_rust": medians["imre"] / medians["rust"],
        "cpython_stdout_sha256": oracle_sha,
        "cpython_stdout_bytes": len(original.stdout),
        "observed_stdout_sha256": observed,
        "environment": env,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "codon", "rust", "output"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--source", type=Path, default=UPSTREAM)
    parser.add_argument("--python", type=Path, default=Path(sys.executable))
    parser.add_argument("--compiler", type=Path)
    parser.add_argument("--runtime", type=Path)
    parser.add_argument("--cpu", type=int, required=True)
    parser.add_argument("--samples", type=int, default=24)
    args = parser.parse_args()
    if sys.flags.optimize or args.samples < 4 or args.samples % 4:
        parser.error(
            "run with assertions enabled and a positive multiple of four samples"
        )
    if args.cpu not in os.sched_getaffinity(0):
        parser.error("the requested CPU is unavailable")
    os.sched_setaffinity(0, {args.cpu})
    source = args.source.resolve(strict=True)
    commands = {"cpython": [str(args.python.absolute()), "-B", str(source)]}
    commands.update(
        {
            name: [str(getattr(args, name).resolve(strict=True))]
            for name in ("codon", "imre", "rust")
        }
    )
    tools = [source, Path(__file__).resolve(), *(Path(v[0]) for v in commands.values())]
    tools.extend(
        p.resolve(strict=True) for p in (args.runtime, args.compiler) if p is not None
    )
    tools.extend(
        p
        for name in ("rust", "codon")
        if (p := Path(commands[name][0]).parent / "build-record.json").is_file()
    )
    result = {
        "platform": platform.platform(),
        "cpu": args.cpu,
        "commands": commands,
        "cpu_model": next(
            l.split(":", 1)[1].strip()
            for l in Path("/proc/cpuinfo").read_text().splitlines()
            if l.startswith("model name")
        ),
        "cpython": subprocess.check_output(
            [commands["cpython"][0], "--version"], text=True
        ).strip(),
        "artifacts": [file_id(path) for path in tools],
        "cases": {},
    }
    with tempfile.TemporaryDirectory(prefix="imre-memory-compare-") as tmp:
        for layout, projects, rows in (
            ("1x", 1, 1),
            ("10x-projects", 10, 1),
            ("10x-rows", 1, 10),
        ):
            for changed in (True, False):
                label = f"{layout}-{'changed' if changed else 'unchanged'}"
                directory = Path(tmp) / label
                oracle, inputs = prepare(directory, projects, rows, changed)
                case = {**inputs, **measure(commands, directory, oracle, args.samples)}
                result["cases"][label] = case
                print(
                    label,
                    {k: round(v, 3) for k, v in case["median_ms"].items()},
                    flush=True,
                )
    assert result["artifacts"] == [file_id(path) for path in tools]
    args.output.write_bytes(
        gzip.compress(json.dumps(result, sort_keys=True).encode(), mtime=0)
    )


if __name__ == "__main__":
    main()
