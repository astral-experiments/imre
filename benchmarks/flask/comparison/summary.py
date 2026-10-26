"""Render medians across rounds from a compare.py receipt."""
import gzip
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path


def table(report):
    grouped = defaultdict(list)
    for sample in report["trials"]:
        grouped[sample["profile"], sample["concurrency"], sample["server"]].append(sample)
    print("| Workload | Clients | Server | Requests/s | p50 ms | p95 ms | p99 ms |")
    print("| --- | ---: | --- | ---: | ---: | ---: | ---: |")
    for (profile, clients, name), samples in sorted(grouped.items()):
        if len(samples) != report["settings"]["rounds"]:
            raise ValueError(f"incomplete {name} {profile} c={clients}")
        rps = statistics.median(s["RPS"] for s in samples)
        percentiles = [statistics.median(s["All"][f"P{p}US"] for s in samples) / 1000 for p in (50, 95, 99)]
        print(f"| {profile} | {clients} | {name} | {rps:,.0f} | " +
              " | ".join(f"{val:.3f}" for val in percentiles) + " |")


if __name__ == "__main__":
    path = Path(sys.argv[1])
    opener = gzip.open if path.suffix == ".gz" else open
    with opener(path, "rt") as src:
        table(json.load(src))
