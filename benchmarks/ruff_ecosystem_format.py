"""Check and time the full Ruff format report on frozen Git patches (Linux)."""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import tempfile

from conformance_regex_benchmark import file_id, orders


ROOT = Path(__file__).resolve().parents[1]
DRIVER = ROOT / "examples/ruff_ecosystem/format_replay.py"


def digest(blob):
    return hashlib.sha256(blob).hexdigest()


def fixture(directory):
    # A file renamed and edited, a new file, a removal, and a file without newline.
    patches = {
        "not-changed": "",
        "edited": (
            "diff --git a/old path b/new path\n"
            "similarity index 88%\nrename from old path\nrename to new path\n"
            "--- a/old path\n+++ b/new path\n@@ -5,2 +5,2 @@ a section\n"
            " context\n-before\n+after\n\\ No newline at end of file\n"
            "diff --git a/π.py b/π.py\nnew file mode 100644\n--- /dev/null\n+++ b/π.py\n"
            "@@ -0,0 +1 @@\n+über\n"
        ),
        "removed": (
            "diff --git a/deleted b/deleted\ndeleted file mode 100644\n"
            "--- a/deleted\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n"
        ),
        "error": "  format failed\nwith stderr\n",
    }
    rows = []
    for name, patch in patches.items():
        (directory / name).write_bytes(patch.encode())
        rows.append(
            "\t".join(
                [
                    "fixture",
                    name,
                    "abcdef",
                    "1" if name == "edited" else "0",
                    "generated" if name == "edited" else "",
                    "error" if name == "error" else "ok",
                    name,
                ]
            )
        )
    (directory / "projects.tsv").write_text("\n".join(rows) + "\n")


def manifest(directory):
    result = []
    rows = (directory / "projects.tsv").read_text().splitlines()
    for row in rows:
        owner, name, revision, preview, exclude, kind, filename = row.split("\t")
        if preview not in {"0", "1"} or kind not in {"ok", "error"}:
            raise ValueError(f"invalid format replay row: {row}")
        data = (directory / filename).read_bytes()
        result.append(
            {
                "name": f"{owner}/{name}",
                "revision": revision,
                "preview": preview == "1",
                "exclude": exclude,
                "kind": kind,
                "bytes": len(data),
                "sha256": digest(data),
            }
        )
    if len(result) != 58 or len({p["name"] for p in result}) != 58:
        raise ValueError("the formatter benchmark requires all 58 unique Ruff targets")
    return result


def run(monitor, command, arguments, env):
    # The small C parent keeps Python's heap out of wait4().ru_maxrss.
    metrics = monitor.with_name("metrics")
    with tempfile.TemporaryFile() as out:
        proc = subprocess.run(
            [monitor, metrics, *command, *arguments],
            stdout=out,
            stderr=subprocess.PIPE,
            env=env,
        )
        if proc.returncode or proc.stderr:
            raise RuntimeError(
                f"{command} {arguments}: exit {proc.returncode}, stderr {proc.stderr[:2000]!r}"
            )
        wall, rss = map(int, metrics.read_text().split())
        out.seek(0)
        return out.read(), {"wall_ns": wall, "peak_rss_kib": rss}


def check(monitor, command, arguments, env, expected):
    output, sample = run(monitor, command, arguments, env)
    if output != expected:
        offset = next(
            (i for i, (a, b) in enumerate(zip(output, expected)) if a != b),
            min(len(output), len(expected)),
        )
        raise ValueError(
            f"{command[0]} differs from upstream CPython at byte {offset}: "
            f"{output[offset : offset + 64]!r} vs {expected[offset : offset + 64]!r}"
        )
    return sample


def verify_projects(monitor, engines, captures, directory, env):
    """Check every project's own report, including each error and no-change case."""
    directory.mkdir()
    (directory / "source").symlink_to(captures.resolve(), target_is_directory=True)
    reports = {}
    for row in (captures / "projects.tsv").read_text().splitlines():
        fields = row.split("\t")
        owner, name = fields[:2]
        fields[-1] = f"source/{fields[-1]}"
        # Keep the replay input format identical, with a one-row manifest.
        (directory / "projects.tsv").write_text("\t".join(fields) + "\n")
        argv = [directory, "1"]
        expected, _ = run(monitor, engines["cpython"], argv, env)
        check(monitor, engines["imre"], argv, env, expected)
        reports[f"{owner}/{name}"] = {
            "bytes": len(expected),
            "sha256": digest(expected),
        }
    return reports


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("captures", "python", "imre", "codon", "rust", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument(
        "--previous-imre", type=Path, help="time the previous replay binary in the same rounds"
    )
    parser.add_argument("--cpu", type=int, required=True)
    parser.add_argument("--rounds", type=int)
    args = parser.parse_args()
    cycle = 10 if args.previous_imre else 4
    if args.rounds is None:
        args.rounds = 10 if args.previous_imre else 8
    if platform.system() != "Linux" or args.rounds < cycle or args.rounds % cycle:
        parser.error(f"need Linux and a multiple of {cycle} measured rounds")
    with tempfile.TemporaryDirectory(prefix="ruff-format-") as tmp:
        measure(args, Path(tmp))


def measure(args, tmp):
    monitor = tmp / "process-usage"
    subprocess.run(
        [
            "cc",
            "-O2",
            "-std=c11",
            ROOT / "benchmarks/catalog_process_usage.c",
            "-o",
            monitor,
        ],
        check=True,
    )
    os.sched_setaffinity(0, {args.cpu})
    env = dict(
        os.environ,
        PYTHONHASHSEED="0",
        PYTHONPATH=str(DRIVER.parent),
        PYTHONNOUSERSITE="1",
    )
    env.pop("PYTHONSAFEPATH", None)
    engines = {
        "cpython": [str(args.python.absolute()), "-B", str(DRIVER)],
        **{
            name: [str(getattr(args, name).resolve())]
            for name in ("imre", "codon", "rust")
        },
    }
    if args.previous_imre:
        engines["previous_imre"] = [str(args.previous_imre.resolve())]
    schedule = orders(list(engines))
    evidence = {
        "platform": platform.platform(),
        "cpu": args.cpu,
        "rounds": args.rounds,
        "cpython": subprocess.check_output(
            [args.python, "--version"], text=True
        ).strip(),
        "unidiff": subprocess.check_output(
            [
                args.python,
                "-c",
                'import importlib.metadata; print(importlib.metadata.version("unidiff"))',
            ],
            text=True,
        ).strip(),
        "source": json.loads((DRIVER.parent / "UPSTREAM.json").read_text())["revision"],
        "artifacts": {
            name: {k: v for k, v in file_id(Path(command[0])).items() if k != "path"}
            for name, command in engines.items()
        },
        "inputs": manifest(args.captures),
        "cases": {},
    }

    edges = tmp / "edges"
    edges.mkdir()
    fixture(edges)
    want, _ = run(monitor, engines["cpython"], [edges, "1"], env)
    for command in engines.values():
        check(monitor, command, [edges, "1"], env, want)
    evidence["individual_reports"] = verify_projects(
        monitor, engines, args.captures, tmp / "individual", env
    )
    for copies in (1, 10):
        arguments = [args.captures.resolve(), str(copies)]
        expected, _ = run(monitor, engines["cpython"], arguments, env)
        samples = {engine: [] for engine in engines}
        for turn in range(-1, args.rounds):
            for name in schedule[turn % len(schedule)]:
                sample = check(monitor, engines[name], arguments, env, expected)
                if turn >= 0:
                    samples[name].append(sample)
        median = {
            name: {key: statistics.median(row[key] for row in rows) for key in rows[0]}
            for name, rows in samples.items()
        }
        ratio = median["imre"]["wall_ns"] / median["rust"]["wall_ns"]
        evidence["cases"][str(copies)] = {
            "bytes": len(expected),
            "sha256": digest(expected),
            "samples": samples,
            "median": median,
            "imre_over_rust": ratio,
        }
        print(
            f"{copies}×: {len(expected):,} matching Markdown bytes, Imre / Rust {ratio:.3f}",
            flush=True,
        )
    data = (json.dumps(evidence, indent=2) + "\n").encode()
    if args.output.suffix == ".gz":
        data = gzip.compress(data, mtime=0)
    args.output.write_bytes(data)


if __name__ == "__main__":
    main()
