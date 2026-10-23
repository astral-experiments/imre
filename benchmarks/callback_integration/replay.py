"""Replay the pinned two-callback quadrature case with two default split release compilers."""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import random
import re
import runpy
import statistics
import subprocess
import time


HERE = Path(__file__).resolve().parent
UPSTREAM_SHA256 = "b24c4536ecbe3c0428f61776d8ae931a51610d426894f84272188a23cebac1e3"
CASES = {"constant": (0, 12.0, 14.0, 1000, 2500), "quadratic-negative": (1, -4.0, 0.0, 10000, 250),
         "quadratic-symmetric": (1, -4.0, 4.0, 10000, 250)}
MODES = ("baseline", "candidate")


def sha(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def output(command, **kwargs):
    result = subprocess.run(command, stdin=subprocess.DEVNULL, capture_output=True, check=True, timeout=90, **kwargs)
    assert not result.stderr, (command, result.stderr)
    return result.stdout


def setup(folder, layout):
    src = folder / "source"
    src.mkdir(parents=True)
    path = HERE / "numerical_integration.py"
    assert sha(path) == UPSTREAM_SHA256
    integration = path.read_text().split('if __name__ == "__main__":')[0]
    driver = (HERE / "driver.py").read_text()
    if layout == "same":
        start, stop = driver.index("def constant("), driver.index("def repeat(")
        integration += driver[start:stop]
        driver = (driver[:start] + driver[stop:]).replace("import trapezoidal_area", "import select, trapezoidal_area")
    (src / "integration.py").write_text(integration)
    (src / "driver.py").write_text(driver)
    return src


def compile_program(cli, runtime, src, folder):
    folder.mkdir()
    app, cache = folder / "app", folder / "cache"
    env = {k: v for k, v in os.environ.items() if k not in ("RELARFLAGS", "RELAR_ENCODED_FLAGS")}
    env["RELAR_CACHE_DIR"] = str(cache)
    output([cli, "build", src / "driver.py", "--module-root", src, "--runtime", runtime, "--quiet", "-o", app], env=env)
    objects = sorted((cache / "v1/objects").glob("*.o"))
    assert len(objects) == 2, "Expected two cached default native objects. Explicit --clang selects whole-program mode."
    symbols = output(["nm", "-S", "--defined-only", app]).decode()
    function = re.search(r"^[0-9a-f]+ ([0-9a-f]+) [tT] fn_m1_0$", symbols, re.M)
    assert function, "The integration module's first declared function must be present"
    text = folder / "section.text"
    output(["objcopy", "--dump-section", f".text={text}", app, folder / "elf-copy"])
    return {"compiler_sha256": sha(cli), "elf_sha256": sha(app), "object_sha256": sorted(sha(p) for p in objects),
            "routine_symbol": "fn_m1_0", "routine_bytes": int(function[1], 16), "text_bytes": text.stat().st_size, "text_sha256": sha(text)}


def oracles():
    integrate = runpy.run_path(str(HERE / "numerical_integration.py"))["trapezoidal_area"]
    result = {}
    for label, (which, a, b, steps, reps) in CASES.items():
        original, typed = ((lambda x: 5), (lambda x: 5.0)) if which == 0 else ((lambda x: 9*x**2), (lambda x: 9*(x*x)))
        value = integrate(original, a, b, steps)
        assert value.hex() == integrate(typed, a, b, steps).hex()
        x = a
        altered = 0
        for _ in range(steps + 1):
            altered += original(x) != typed(x)
            x += (b-a)/steps
        total = 0.0
        for _ in range(reps):
            total += value
        result[label] = {"single": value.hex(), "work": total.hex(), "zero": (0.0).hex(), "changed_single_callback_results": altered}
    return result


def measure(app, label, n, expected):
    which, a, b, steps, reps = CASES[label]
    ref = {0: "zero", 1: "single", reps: "work"}[n]
    start = time.perf_counter_ns()
    actual = output([app, *map(str, (which, a, b, steps, n))])
    elapsed = (time.perf_counter_ns() - start) / 1_000_000
    assert float(actual).hex() == expected[label][ref], (label, actual, expected[label][ref])
    return elapsed


def benchmark(folder, expected, blocks):
    rng = random.Random(2901)
    orders, rows = {}, {case: [] for case in CASES}
    for case in CASES:
        orders[case] = []
        for _ in range(blocks//2):
            pair = rng.sample(MODES, 2)
            orders[case].extend((pair, pair[::-1]))
        for mode in MODES:
            measure(folder / mode / "app", case, CASES[case][-1], expected)
            measure(folder / mode / "app", case, 0, expected)
    for block in range(blocks):
        for case in rng.sample(list(CASES), len(CASES)):
            row = {"order": orders[case][block], "work_ms": {}, "zero_ms": {}}
            for mode in row["order"]:
                for n in rng.sample((0, CASES[case][-1]), 2):
                    row["work_ms" if n else "zero_ms"][mode] = measure(folder / mode / "app", case, n, expected)
            rows[case].append(row)
    return rows


def summarize(rows, label):
    ratios = [r["work_ms"]["baseline"] / r["work_ms"]["candidate"] for r in rows]
    pairs = list(zip(ratios[::2], ratios[1::2], strict=True))
    assert all(a["order"] == b["order"][::-1] for a, b in zip(rows[::2], rows[1::2], strict=True))
    rng = random.Random("trapezoidal-area/" + label)
    boot = sorted(statistics.median(value for pair in rng.choices(pairs, k=len(pairs)) for value in pair) for _ in range(10000))
    return {"baseline_over_candidate": statistics.median(ratios), "bootstrap95": [boot[249], boot[9749]],
            "candidate_over_baseline": statistics.median(1 / r for r in ratios), "faster_blocks": sum(r > 1 for r in ratios),
            "median_work_ms": {mode: statistics.median(r["work_ms"][mode] for r in rows) for mode in MODES}}


def audit(path):
    with gzip.open(path, "rt") as file:
        record = json.load(file)
    assert record["pinned_upstream_source_sha256"] == sha(HERE / "numerical_integration.py") == UPSTREAM_SHA256
    assert record["oracles"] == oracles()
    for case, values in record["cases"].items():
        assert list(CASES[case]) == values
    for study in (record["layouts"]["same"], *record["alternative_controls"].values()):
        for label, rows in study["intervals"].items():
            assert len(rows) == 24
            assert study["summary"][label] == summarize(rows, label)
    b, c = (record["layouts"]["cross"]["builds"][mode] for mode in MODES)
    assert b["elf_sha256"] == c["elf_sha256"] and b["object_sha256"] == c["object_sha256"]
    print("Verified source and all upstream outputs, paired summaries and confidence intervals, and excluded-layout binary identity.")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    for key in ("baseline", "candidate", "runtime", "work", "out"):
        ap.add_argument("--" + key, type=Path)
    ap.add_argument("--audit-record", type=Path)
    ap.add_argument("--blocks", type=int, default=24)
    args = ap.parse_args()
    if args.audit_record:
        audit(args.audit_record)
        return
    if any(getattr(args, k) is None for k in ("baseline", "candidate", "runtime", "work", "out")):
        ap.error("Supply both compilers, their common runtime, a fresh work directory and output file.")
    assert args.blocks >= 0 and args.blocks % 2 == 0
    work = args.work.resolve()
    assert not work.exists(), "Supply a fresh work directory for independent cache proof"
    expected = oracles()
    result = {"pinned_upstream_source_sha256": UPSTREAM_SHA256, "runtime_sha256": sha(args.runtime), "cases": CASES,
              "oracles": expected, "blocks": args.blocks, "logical_cpus": sorted(os.sched_getaffinity(0)), "layouts": {}}
    for layout in ("cross", "same"):
        folder = work / layout
        src = setup(folder, layout)
        builds = {mode: compile_program(getattr(args,mode).resolve(),args.runtime.resolve(),src,folder/mode) for mode in MODES}
        for mode in MODES:
            for case in CASES:
                for n in (0, 1, CASES[case][-1]):
                    measure(folder/mode/"app",case,n,expected)
        record = {"builds": builds}
        if layout == "cross":
            for field in ("elf_sha256", "object_sha256"):
                assert builds["baseline"][field] == builds["candidate"][field], (layout, field)
        elif args.blocks:
            raw = benchmark(folder, expected, args.blocks)
            record["intervals"] = raw
            record["summary"] = {k: summarize(rows, k) for k, rows in raw.items()}
        result["layouts"][layout] = record
    args.out.write_text(json.dumps(result, indent=2) + "\n")
    print("Verified both layouts against the upstream integrals and repeated checksums; cross-unit objects and complete ELF matched.")


if __name__ == "__main__":
    main()
