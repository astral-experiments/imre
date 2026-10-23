"""Independently recompute the six-engine pairing, output checks, and Rust gates."""

import argparse
import json
import sys
from collections import Counter
from hashlib import sha256
from itertools import pairwise
from pathlib import Path
from statistics import median

ENGINES = {"upstream", "cpython", "before", "after", "codon", "rust"}


def digest(path):
    return sha256(path.read_bytes()).hexdigest()


def audit(root, verify_host=False):
    data = json.loads((root / "results.json").read_bytes())
    assert data["completed"] and data["schema"] == 1 and not data["pilot"]
    n = data["samples_per_engine"]
    assert n >= 12 and n % len(ENGINES) == 0
    cfg = data["config"]
    assert (
        set(cfg["engines"]) == ENGINES
        and len(cfg["revision"]) == len(cfg["baseline_revision"]) == 40
    )
    schedule = data["order"]
    assert len(schedule) == len(ENGINES) and all(set(r) == ENGINES for r in schedule)
    assert all(
        Counter(r[i] for r in schedule) == Counter(ENGINES) for i in range(len(ENGINES))
    )
    assert Counter(p for row in schedule for p in pairwise(row)) == Counter(
        (a, b) for a in ENGINES for b in ENGINES if a != b
    )
    expected_cases = set()
    panels = set()
    observations = 0
    ratios = {}
    for scale in (1, 10):
        for mode in ("group", "files", "equivalent", "compute"):
            expected_cases.add(f"{scale}x/changed-github/{mode}")
        for state in ("changed", "unchanged"):
            for fmt in ("diff", "github"):
                expected_cases.add(f"{scale}x/{state}-{fmt}/report")
                filename = root / f"{scale}x-{state}-{fmt}.md"
                key = f"{scale}x/{state}-{fmt}/report"
                assert data["cases"][key]["oracle"]["1"][1] == digest(filename)
            for mode in ("dump", "aliases"):
                name = f"{scale}x-{state}-{mode}"
                panels.add(name)
                meta = data["parity"][name]
                f = root / (name + ".txt")
                h = digest(f)
                assert h == meta["oracle"]["sha256"] and meta["engines"] == {
                    e: h for e in ENGINES
                }
                if mode == "dump":
                    rows = f.read_bytes().splitlines()
                    counts = Counter(v[:1] for v in rows)
                    group_count, diagnostic_count = (
                        (1269, 2328) if state == "changed" else (1228, 2360)
                    )
                    assert counts == {
                        b"F": 146 * scale,
                        b"G": group_count * scale,
                        b"E": 1323 * scale,
                        b"D": diagnostic_count * scale,
                    }
                    keys = [v.split(b"\t")[1] for v in rows if v.startswith(b"G\t")]
                    assert keys == sorted(set(keys))
                else:
                    retained, empty, remaining = map(int, f.read_bytes().split())
                    assert retained == 1 and empty - remaining == 1
    assert set(data["parity"]) == panels and set(data["cases"]) == expected_cases
    for name, case in data["cases"].items():
        paired = not name.endswith("/report")
        assert paired == case["paired"]
        rounds = case["rounds"]
        assert rounds > 0
        assert len(case["samples"]) == n
        times = {e: [] for e in ENGINES}
        for i, row in enumerate([case["warmup"], *case["samples"]], -1):
            assert [v["engine"] for v in row] == schedule[i % len(schedule)]
            for pos, v in enumerate(row):
                obs = v["engine"]
                sequence = [0, rounds] if paired else [rounds]
                if (i + pos) % 2:
                    sequence.reverse()
                assert v["execution_order"] == sequence
                for rep in sequence:
                    s = v["measure" if rep else "zero"]
                    expected = case["oracle"][str(rep)]
                    assert [s["stdout"], s["output"]] == expected and s["ns"] > 0
                    observations += 1
                if i >= 0:
                    times[obs].append(
                        (v["measure"]["ns"] - v["zero"]["ns"]) / rounds
                        if paired
                        else v["measure"]["ns"]
                    )
        med = {e: median(v) for e, v in times.items()}
        assert med == case["median_ns"] and min(med.values()) > 0
        ratios[name] = med["after"] / med["rust"]
    assert data["ratios"] == ratios and data["time_target_met"] == all(
        v <= 2 for v in ratios.values()
    )
    if verify_host:
        for f in cfg["build_records"] + cfg["guarded_inputs"]:
            assert (
                digest(Path(f["path"])) == f["sha256"]
                and Path(f["path"]).stat().st_size == f["bytes"]
            )
    return {
        "mode": cfg["mode"],
        "revision": cfg["revision"],
        "baseline_revision": cfg["baseline_revision"],
        "cases": len(expected_cases),
        "observations": observations,
        "parity_streams": len(panels),
        "time_target_met": data["time_target_met"],
        "largest_ratio": max(ratios.values()),
        "results_sha256": digest(root / "results.json"),
    }


if __name__ == "__main__":
    if sys.flags.optimize:
        raise SystemExit("audit requires assertions")
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("directory", type=Path)
    ap.add_argument("--verify-host", action="store_true")
    a = ap.parse_args()
    r = audit(a.directory, a.verify_host)
    print(json.dumps(r, indent=2))
    sys.exit(0 if r["time_target_met"] else 1)
