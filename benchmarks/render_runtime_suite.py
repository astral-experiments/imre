# /// script
# requires-python = ">=3.11"
# dependencies = ["matplotlib==3.10.8"]
# ///
"""Render the runtime suite's SVG and print its two Markdown tables."""

import argparse
import json
import math
import statistics
from pathlib import Path

import matplotlib as mpl
import matplotlib.pyplot as plt
from matplotlib.ticker import NullLocator

STYLES = {
    "rust": ("Rust", "#45505f", "s"),
    "cpython": ("CPython", "#326a9c", "o"),
    "relar": ("Relar", "#ac4429", "D"),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("results", type=Path)
    parser.add_argument("--svg", type=Path, required=True)
    args = parser.parse_args()
    cases = json.loads(args.results.read_bytes())["cases"]
    mpl.rcParams.update(
        {
            "font.family": "DejaVu Sans",
            "font.size": 10,
            "svg.fonttype": "none",
            "svg.hashsalt": "relar-runtime-suite",
        }
    )
    fig, ax = plt.subplots(figsize=(8.8, 1.7 + 0.50 * len(cases)))
    fig.subplots_adjust(left=0.235, right=0.97, bottom=0.16, top=0.77)
    fig.text(
        0.025,
        0.91,
        "Runtime by workload",
        fontsize=16,
        weight="semibold",
        color="#202730",
    )
    fig.text(
        0.025,
        0.85,
        f"Median of {len(cases[0]['samples_ns'])} runs  ·  whiskers span the middle 50%",
        fontsize=10,
        color="#46515c",
    )
    times = [
        "| Workload | Input per process | Rust (ms) | CPython (ms) | Relar (ms) | CPython / Relar |",
        "| --- | --- | ---: | ---: | ---: | ---: |",
    ]
    spreads = [
        "| Workload | Rust (ms) | CPython (ms) | Relar (ms) | Output file (bytes) |",
        "| --- | ---: | ---: | ---: | ---: |",
    ]
    limits = []
    for row, case in enumerate(cases):
        medians, intervals = {}, []
        for offset, (engine, (label, color, marker)) in zip(
            (-0.19, 0, 0.19), STYLES.items(), strict=True
        ):
            samples = [r[engine] / 1e6 for r in case["samples_ns"]]
            low, mid, high = statistics.quantiles(samples, n=4, method="inclusive")
            medians[engine] = mid
            intervals.append(f"{low:.2f}–{high:.2f}")
            limits.extend([low, high])
            ax.errorbar(
                mid,
                row + offset,
                xerr=[[mid - low], [high - mid]],
                fmt=marker,
                color=color,
                markersize=5.5,
                elinewidth=1.4,
                capsize=2.5,
                label=label if row == 0 else None,
                zorder=3,
            )
        cells = " | ".join(f"{mid:.2f}" for mid in medians.values())
        speedup = medians["cpython"] / medians["relar"]
        times.append(f"| {case['name']} | {case['input']} | {cells} | {speedup:.2f}× |")
        spreads.append(
            f"| {case['name']} | {' | '.join(intervals)} | {case['report_bytes']:,} |"
        )

    ax.set_xscale("log")
    lo, hi = min(limits) / 1.25, max(limits) * 1.25
    ticks = [
        n * 10**e
        for e in range(math.floor(math.log10(lo)), math.ceil(math.log10(hi)) + 1)
        for n in (1, 2, 5)
        if lo <= n * 10**e <= hi
    ]
    ax.set_xticks(ticks, [f"{x:g}" for x in ticks])
    ax.xaxis.set_minor_locator(NullLocator())
    ax.set_xlim(lo, hi)
    ax.set_ylim(len(cases) - 0.45, -0.45)
    ax.set_yticks(range(len(cases)), [c["name"] for c in cases])
    ax.tick_params(axis="both", length=0, pad=6, colors="#394450")
    ax.set_xlabel(
        "Elapsed time (ms, log scale)  ·  lower is faster", color="#46515c", labelpad=9
    )
    ax.grid(axis="x", color="#dce1e6", linewidth=0.7)
    for row in range(1, len(cases)):
        ax.axhline(row - 0.5, color="#edf0f3", linewidth=0.65)
    ax.spines[:].set_visible(False)
    ax.legend(
        loc="lower right",
        bbox_to_anchor=(1.0, 1.045),
        ncols=3,
        frameon=False,
        fontsize=9,
        handlelength=1.15,
        columnspacing=1.2,
    )
    args.svg.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(
        args.svg, format="svg", metadata={"Date": None, "Title": "Runtime by workload"}
    )
    plt.close(fig)
    print("\n".join(times), "\n\n", "\n".join(spreads), sep="")


if __name__ == "__main__":
    main()
