"""Capture, verify, and time Ruff's frozen ecosystem diagnostics on Linux."""

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

from conformance_regex_benchmark import file_id, orders

HERE = Path(__file__).resolve().parent
SOURCE = HERE.parent / "examples/ruff_ecosystem/replay.py"
CORPUS = json.loads((HERE / "fixtures/ruff_ecosystem/corpus.json").read_text())


def verify(path, digest):
    with path.open("rb") as file:
        actual = hashlib.file_digest(file, "sha256").hexdigest()
    if actual != digest:
        raise ValueError(f"{path} has SHA-256 {actual}, expected {digest}")


def capture(args):
    args.output.mkdir(parents=True, exist_ok=True)
    for binary, version in zip(
        (args.old_ruff, args.new_ruff), CORPUS["ruff_versions"], strict=True
    ):
        if (
            subprocess.check_output([binary, "--version"], text=True).strip()
            != f"ruff {version}"
        ):
            raise ValueError(f"expected Ruff {version} at {binary}")
    for project in CORPUS["projects"]:
        checkout = args.checkouts / project["name"]
        if (
            subprocess.check_output(
                ["git", "rev-parse", "HEAD"], cwd=checkout, text=True
            ).strip()
            != project["revision"]
        ):
            raise ValueError(f"expected {project['revision']} at {checkout}")
        for binary, version, digest in zip(
            (args.old_ruff, args.new_ruff),
            CORPUS["ruff_versions"],
            project["sha256"],
            strict=True,
        ):
            path = args.output / f"{project['name']}-{version}.stdout"
            with path.open("wb") as stdout:
                subprocess.run(
                    [binary.resolve(), *CORPUS["flags"]],
                    cwd=checkout,
                    stdout=stdout,
                    check=True,
                )
            verify(path, digest)
    prepare(args.output, args.output)


def prepare(captures, directory):
    rows = []
    hashes = {}
    for project in CORPUS["projects"]:
        files = []
        for version, digest in zip(
            CORPUS["ruff_versions"], project["sha256"], strict=True
        ):
            name = f"{project['name']}-{version}.stdout"
            source = (captures / name).resolve()
            verify(source, digest)
            if captures != directory:
                (directory / name).symlink_to(source)
            files.append(name)
            hashes[name] = digest
        rows.append(
            "\t".join([project["owner"], project["name"], project["revision"], *files])
        )
    (directory / "projects.tsv").write_text("\n".join(rows) + "\n")
    return hashes


def adversarial(directory):
    """Exercise the rows discarded by the reporter as well as the visible ones."""
    directory.mkdir()
    stable = [f"z{i}.py:1:1: F400 padding" for i in range(198)]
    a, b, c = [f"x.py:1:1: {r} same message" for r in ("A001", "B001", "C001")]
    common = ["c.py:1:2: B405 same", "", "Found 100 errors.", "3 fixable with foo"]
    pairs = [
        (common, common),
        (
            [
                "dir/π.py:0:1: W123 [*] note",
                "a.py:1:2: has-name: old",
                "unparsed",
                *common,
            ],
            [
                "dir/π.py:0:1: W123 note",
                "a.py:1:2: has-name: new",
                "f.py:8:1: A001 a",
                *common,
            ],
        ),
        ([b] * 5 + [c] * 5 + stable, [a] * 5 + [b] * 5 + stable),
        ([b, b], [b, c]),
    ]
    rows = []
    for i, (old, new) in enumerate(pairs):
        paths = [f"{i}.old", f"{i}.new"]
        for name, values in zip(paths, (old, new), strict=True):
            (directory / name).write_bytes(("\r\n".join(values) + "\r\n").encode())
        rows.append("\t".join(["fixture", f"project-{i}", "abcdef", *paths]))
    (directory / "projects.tsv").write_text("\n".join(rows) + "\n")


def run(command, arguments, env):
    with tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as errors:
        started = time.perf_counter_ns()
        child = subprocess.Popen(
            [*command, *arguments], stdout=output, stderr=errors, env=env
        )
        _, status, usage = os.wait4(child.pid, 0)
        elapsed = time.perf_counter_ns() - started
        child.returncode = os.waitstatus_to_exitcode(status)
        errors.seek(0)
        stderr = errors.read()
        if child.returncode or stderr:
            raise RuntimeError(
                f"{command} {arguments}: exit {child.returncode}, stderr {stderr[:2000]!r}"
            )
        output.seek(0)
        return output.read(), {"wall_ns": elapsed, "peak_rss_kib": usage.ru_maxrss}


def measure(args):
    if platform.system() != "Linux" or args.rounds < 4 or args.rounds % 4:
        raise ValueError("use Linux and a multiple of four measured rounds")
    os.sched_setaffinity(0, {args.cpu})
    env = dict(
        os.environ,
        PYTHONHASHSEED="0",
        PYTHONPATH=str(SOURCE.parent),
        PYTHONNOUSERSITE="1",
    )
    commands = {
        "cpython": [str(args.python.absolute()), "-B", str(SOURCE)],
        **{
            name: [str(getattr(args, name).resolve())]
            for name in ("relar", "codon", "rust")
        },
    }
    result = {
        "commands": commands,
        "cpu": args.cpu,
        "platform": platform.platform(),
        "cpython": subprocess.check_output(
            [args.python, "--version"], text=True
        ).strip(),
        "artifacts": [file_id(Path(command[0])) for command in commands.values()],
        "cases": {},
    }
    with tempfile.TemporaryDirectory(prefix="ruff-ecosystem-") as tmp:
        directory = Path(tmp)
        result["inputs"] = prepare(args.captures, directory)
        edges = directory / "edges"
        adversarial(edges)
        edge_oracle, _ = run(commands["cpython"], [edges, "1"], env)
        for name, command in commands.items():
            if run(command, [edges, "1"], env)[0] != edge_oracle:
                raise ValueError(
                    f"{name} differs on duplicate, fix-only or malformed diagnostics"
                )
        schedule = orders(list(commands))
        for copies in ("1", "10"):
            arguments = [directory, copies]
            record = CORPUS["reports"][copies]
            oracle, _ = run(commands["cpython"], arguments, env)
            if (
                len(oracle) != record["bytes"]
                or hashlib.sha256(oracle).hexdigest() != record["sha256"]
            ):
                raise ValueError(
                    f"upstream CPython diverged from the frozen {copies}× report"
                )
            samples = {name: [] for name in commands}
            for round_ in range(-1, args.rounds):
                for name in schedule[round_ % len(schedule)]:
                    output, sample = run(commands[name], arguments, env)
                    if output != oracle:
                        raise ValueError(
                            f"{name} {copies}× round {round_} produced different Markdown"
                        )
                    if round_ >= 0:
                        samples[name].append(sample)
            medians = {
                name: {
                    key: statistics.median(s[key] for s in values) for key in values[0]
                }
                for name, values in samples.items()
            }
            ratio = medians["relar"]["wall_ns"] / medians["rust"]["wall_ns"]
            result["cases"][copies] = {
                **record,
                "samples": samples,
                "median": medians,
                "relar_over_rust": ratio,
            }
            print(f"{copies}× Relar / Rust {ratio:.3f}", flush=True)
    with gzip.GzipFile(filename=str(args.output), mode="wb", mtime=0) as output:
        output.write((json.dumps(result, indent=2) + "\n").encode())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subs = parser.add_subparsers(dest="command", required=True)
    cap = subs.add_parser("capture")
    for name in ("old-ruff", "new-ruff", "checkouts", "output"):
        cap.add_argument("--" + name, type=Path, required=True)
    bench = subs.add_parser("bench")
    for name in ("captures", "relar", "codon", "rust", "output"):
        bench.add_argument("--" + name, type=Path, required=True)
    bench.add_argument("--python", type=Path, default=Path(sys.executable))
    bench.add_argument("--cpu", type=int, required=True)
    bench.add_argument("--rounds", type=int, default=4)
    args = parser.parse_args()
    (capture if args.command == "capture" else measure)(args)


if __name__ == "__main__":
    main()
