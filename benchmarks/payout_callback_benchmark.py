"""Verify the payout performance record or replay its schedule with two native binaries."""

import argparse
import gzip
import hashlib
import json
import os
import random
import runpy
import statistics
import subprocess
import sys
import time
from pathlib import Path
from types import SimpleNamespace


def summary(pairs, seed, key):
    ratios = [base / cand for base, cand in pairs]
    balanced = list(zip(ratios[::2], ratios[1::2], strict=True))
    rng = random.Random(f"payout/{seed}/{key}")
    boot = sorted(
        statistics.median(
            [x for pair in rng.choices(balanced, k=len(balanced)) for x in pair]
        )
        for _ in range(10000)
    )
    floor = {"gross-full-r256": 1.04, "net-full-r256": 1.025}.get(key, 1 / 1.01)
    lo, hi = boot[249], boot[9749]
    return {
        "median": statistics.median(ratios),
        "bootstrap95": [lo, hi],
        "gate_speedup_floor": floor,
        "gate": "pass" if lo > floor else "fail" if hi < floor else "inconclusive",
    }


def sha(p):
    return hashlib.sha256(p.read_bytes()).hexdigest()


def main():
    if sys.flags.optimize:
        raise SystemExit("replay verification requires Python assertions")
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--record", type=Path, required=True)
    for option in ("repo", "baseline", "candidate", "work", "output"):
        p.add_argument("--" + option, type=lambda s: Path(s).resolve())
    p.add_argument("--cpu", type=int)
    p.add_argument("--check-only", action="store_true")
    a = p.parse_args()
    record = json.loads(gzip.decompress(a.record.read_bytes()))
    study = record["measurements"]
    for key, cell in study["cells"].items():
        assert len(cell["raw_ns"]) == len(cell["baseline_first"]) == study["blocks"]
        assert all(
            l != r
            for l, r in zip(
                cell["baseline_first"][::2], cell["baseline_first"][1::2], strict=True
            )
        )
        assert summary(cell["raw_ns"], study["seed"], key) == cell["speedup"]
        replica = record["replication"]["cells"][key]
        assert len(replica["raw_ns"]) == study["blocks"]
        assert summary(replica["raw_ns"], study["seed"], key) == replica["speedup"]
    assert all(set(order) == set(study["cells"]) for order in study["case_order"])
    assert (
        sum(2 * len(c["raw_ns"]) for c in study["cells"].values())
        == study["timed_processes"]
    )
    if a.baseline is None:
        print(
            f"Verified {2 * study['timed_processes']} observations across both CPUs."
        )
        return
    if not all((a.candidate, a.repo, a.work, a.output)) or a.cpu is None:
        p.error(
            "Replay requires --repo, --baseline, --candidate, --work, --output and --cpu"
        )
    os.sched_setaffinity(0, {a.cpu})
    b = SimpleNamespace(
        **runpy.run_path(str(a.repo / "benchmarks/record_benchmark.py"))
    )
    v = b.validator
    exes = [a.baseline, a.candidate]
    assert {f.name: sha(f) for f in v.PACKAGE.glob("*.py")} == record["source_sha256"]
    validated = v.validate_commands(
        {"baseline": [str(exes[0])], "candidate": [str(exes[1])]}
    )
    assert validated["cases"] == record["validation_cases_per_arm"]
    cells = {}
    a.work.mkdir(parents=True, exist_ok=True)
    for item in b.workloads([32, 4096], 1048576):
        if item["records"] != 4096:
            continue
        original = v.case_from_record(item["case"])
        for rep in (1, 256):
            key = item["label"].removesuffix("-4096") + f"-r{rep}"
            value = v.Case(key, original.files, (*original.arguments[:-1], str(rep)))
            expected = v.oracle(value)
            want = record["inputs"][key]
            assert expected["stdout"] == want["stdout"]
            assert {
                k: hashlib.sha256(val).hexdigest()
                for k, val in expected["files"].items()
            } == want["file_sha256"]
            folder = a.work / key
            folder.mkdir(exist_ok=True)
            v.materialize(value, folder)
            cells[key] = (value, expected, folder)

    def sample(key, arm):
        value, expected, folder = cells[key]
        (folder / "report.txt").write_bytes(b"replace this complete report")
        start = time.perf_counter_ns()
        proc = subprocess.run(
            [exes[arm], *value.arguments],
            input=b"",
            capture_output=True,
            cwd=folder,
            timeout=60,
            check=False,
        )
        ns = time.perf_counter_ns() - start
        v.check_result(value, expected, proc, folder)
        return ns

    new = {key: [] for key in cells}
    for key in cells:
        for arm in study["cells"][key]["warmup_order"]:
            sample(key, arm)
    results = {
        "cpu": a.cpu,
        "binary_sha256": [sha(p) for p in exes],
        "validation_cases_per_arm": validated["cases"],
        "checked_workload_warmups": len(cells) * 2,
    }
    if not a.check_only:
        for block, order in enumerate(study["case_order"]):
            for key in order:
                pair = [0, 0]
                for arm in (
                    (0, 1) if study["cells"][key]["baseline_first"][block] else (1, 0)
                ):
                    pair[arm] = sample(key, arm)
                new[key].append(pair)
        results["cells"] = {
            key: {"raw_ns": raw, "speedup": summary(raw, study["seed"], key)}
            for key, raw in new.items()
        }
    a.output.write_text(json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    main()
