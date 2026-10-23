"""Rebuild three ordinary telemetry binaries and replay the checked report measurements."""

import argparse
import gzip
import hashlib
import itertools
import json
import os
import random
import runpy
import shutil
import statistics
import subprocess
import time
from pathlib import Path
from types import SimpleNamespace


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in (
        "repo",
        "record",
        "baseline",
        "candidate",
        "runtime",
        "lld",
        "work",
        "output",
    ):
        p.add_argument("--" + name, type=lambda arg: Path(arg).resolve(), required=True)
    p.add_argument(
        "--cpu",
        type=int,
        required=True,
        help="available Linux CPU for all timed processes",
    )
    p.add_argument(
        "--schedule",
        type=int,
        choices=(12, 20),
        default=12,
        help="recorded balanced schedule to replay",
    )
    p.add_argument(
        "--check-only",
        action="store_true",
        help="compile all arms and check one run of every case",
    )
    a = p.parse_args()
    meta = json.loads(gzip.decompress(a.record.read_bytes()))
    v = SimpleNamespace(
        **runpy.run_path(str(a.repo / "benchmarks/telemetry_validate.py"))
    )
    a.work.mkdir(parents=True, exist_ok=True)
    hand = a.work / "hand-source" / "telemetry_comparison"
    shutil.copytree(v.PACKAGE, hand, dirs_exist_ok=True)
    subprocess.run(
        ["patch", "--quiet", "--batch", "--fuzz=0", "-p1"],
        cwd=hand,
        input=meta["hand_patch"].encode(),
        check=True,
    )
    assert {p.name: digest(p) for p in v.PACKAGE.glob("*.py")} == meta["inputs"][
        "source_sha256"
    ]
    assert digest(hand / "compare.py") == meta["inputs"]["hand_compare_sha256"]
    env = {
        k: x
        for k, x in os.environ.items()
        if k not in ("RELARFLAGS", "RELAR_ENCODED_FLAGS")
    }
    env["XDG_CONFIG_HOME"] = str(a.work / "empty-config")
    exes = {}
    modes = ("baseline", "automatic", "hand")
    for mode in modes:
        directory = a.work / mode
        directory.mkdir(exist_ok=True)
        compiler = directory / "relar"
        shutil.copy2(a.candidate if mode == "automatic" else a.baseline, compiler)
        shutil.copy2(a.lld, directory / "relar-ld.lld")
        package = hand if mode == "hand" else v.PACKAGE
        exe = directory / "app"
        subprocess.run(
            [
                compiler,
                "build",
                "--quiet",
                package / "__main__.py",
                "--module-root",
                package.parent,
                "--runtime",
                a.runtime,
                "-o",
                exe,
            ],
            check=True,
            env={**env, "RELAR_CACHE_DIR": str(directory / "cache")},
        )
        exes[mode] = exe
    rng = random.Random(meta["inputs"]["seed"])
    n = meta["inputs"]["pairs"]
    left = [(1000 + i * 10, rng.randrange(-10000, 10001)) for i in range(n)]
    right = [(t, x + (i * 13 + 5) % 19 - 9) for i, (t, x) in enumerate(left)]
    schedule = meta["timings"][str(a.schedule)]
    cases = {}
    raw = {}
    for count, info in meta["inputs"]["cases"].items():
        rep = int(count)
        q = v.case(
            f"report-mixed-{rep}",
            left,
            right,
            "report",
            meta["inputs"]["threshold"],
            rep,
        )
        expected = v.oracle(q)
        directory = a.work / f"inputs-{rep}"
        directory.mkdir(exist_ok=True)
        v.materialize(q, directory)
        assert (
            expected["stdout"] == info["expected_stdout"]
            and {k: hashlib.sha256(x).hexdigest() for k, x in expected["files"].items()}
            == info["sha256"]
        )
        cases[rep] = (q, expected, directory)
        raw[rep] = []
    os.sched_setaffinity(0, {a.cpu})

    def sample(rep, mode):
        q, expected, directory = cases[rep]
        (directory / "report.txt").write_bytes(b"replace this complete report")
        start = time.perf_counter_ns()
        proc = subprocess.run(
            [exes[mode], *q.arguments],
            cwd=directory,
            capture_output=True,
            timeout=90,
            check=False,
        )
        elapsed = time.perf_counter_ns() - start
        v.check_result(q, expected, proc, directory)
        return elapsed

    # Derive warmup orders exactly as the original runner did before its timed block schedule.
    rng = random.Random(schedule["schedule_seed"])
    perms = list(itertools.permutations(modes))
    for rep in cases:
        orders = [
            list(order)
            for _ in range(schedule["blocks"] // len(perms))
            for order in rng.sample(perms, len(perms))
        ]
        assert orders == schedule["cases"][str(rep)]["orders"]
        for m in rng.sample(modes, len(modes)):
            sample(rep, m)
    if a.check_only:
        a.output.write_text(
            json.dumps(
                {
                    "elf_sha256": {m: digest(x) for m, x in exes.items()},
                    "verified_runs": len(modes) * len(cases),
                },
                indent=2,
            )
            + "\n"
        )
        return
    for block, counts in enumerate(schedule["case_order"]):
        for rep in counts:
            raw[rep].append(
                {
                    m: sample(rep, m)
                    for m in schedule["cases"][str(rep)]["orders"][block]
                }
            )
    results = {}
    for rep, rows in raw.items():
        ratios = {}
        for key in ("baseline/automatic", "baseline/hand", "hand/automatic"):
            left, right = key.split("/")
            paired = [r[left] / r[right] for r in rows]
            rng = random.Random(f"{schedule['schedule_seed']}/{rep}/{left}/{right}")
            boot = sorted(
                statistics.median(rng.choices(paired, k=len(paired)))
                for _ in range(4000)
            )
            ratios[key] = [statistics.median(paired), boot[100], boot[3899]]
        results[rep] = {
            "raw_ns": rows,
            "median_ms": {
                m: statistics.median(row[m] for row in rows) / 1e6 for m in modes
            },
            "ratios": ratios,
        }
    data = {
        "cpu": a.cpu,
        "elf_sha256": {m: digest(exe) for m, exe in exes.items()},
        "runtime_sha256": digest(a.runtime),
        "lld_sha256": digest(a.lld),
        "cases": results,
    }
    a.output.write_text(json.dumps(data, indent=2) + "\n")
    print({r: d["ratios"] for r, d in results.items()})


if __name__ == "__main__":
    main()
