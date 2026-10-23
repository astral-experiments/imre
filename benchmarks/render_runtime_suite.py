# /// script
# requires-python = ">=3.11"
# dependencies = ["matplotlib==3.10.8"]
# ///
"""Render the README and benchmark-guide figures and print their Markdown tables."""

import argparse
import json
import statistics
from pathlib import Path

import matplotlib as mpl
import matplotlib.pyplot as plt
from matplotlib.ticker import MaxNLocator

ENGINES = {"rust": "Rust", "relar": "Relar", "cpython": "CPython"}
PANEL_TITLES = {
    "Inventory (repeated)": "Inventory · repeated products",
    "Inventory (distinct)": "Inventory · distinct products",
    "Conformance report": "Offline conformance report",
}
INK, MUTED, RULE = "#262528", "#56545b", "#e2dfe1"


def canvas(width, height):
    fig = plt.figure(figsize=(width / 96, height / 96), dpi=96)
    ax = fig.add_axes((0, 0, 1, 1), xlim=(0, width), ylim=(height, 0))
    ax.set_axis_off()
    return fig, ax


def label(ax, x, y, text, size, *, bold=False, color=INK, align="left"):
    ax.text(
        x,
        y,
        text,
        fontsize=size,
        weight="bold" if bold else "normal",
        color=color,
        va="center",
        ha=align,
    )


def save(fig, path, title):
    fig.savefig(path, format="svg", metadata={"Date": None, "Title": title})
    plt.close(fig)
    # Matplotlib leaves trailing spaces at line breaks inside SVG paths.
    path.write_text(path.read_text().replace(" \n", "\n"))


def conformance_chart(case, medians, path):
    fig, ax = canvas(800, 242)
    title = "Render the conformance report"
    label(ax, 26, 29, title, 18, bold=True)
    label(
        ax,
        26,
        56,
        f"{case['input']}  ·  Median of {len(case['samples_ns'])} runs  ·  Lower is faster",
        9.9,
        color=MUTED,
    )
    start, width = 105, 554
    ticks = MaxNLocator(nbins=5, steps=[1, 2, 2.5, 5, 10]).tick_values(
        0, max(medians.values())
    )
    for tick in ticks:
        x = start + width * tick / ticks[-1]
        ax.vlines(x, 79, 185, color=RULE, linewidth=0.7, zorder=0)
        label(ax, x, 205, f"{tick:g}", 8.6, color=MUTED, align="center")
    for i, (engine, name) in enumerate(ENGINES.items()):
        y = 97 + 35 * i
        ms = medians[engine]
        length = width * ms / ticks[-1]
        label(ax, start - 12, y, name, 10.8, align="right")
        ax.barh(y, length, left=start, height=19, color="#edb846", linewidth=0)
        label(ax, start + length + 10, y, f"{ms:.2f} ms", 10.4, bold=True)
    label(
        ax,
        start + width / 2,
        227,
        "Elapsed time (ms)",
        8.8,
        color=MUTED,
        align="center",
    )
    save(fig, path, title)


def workload_panels(rows, path):
    height = 125 + 137 * ((len(rows) + 1) // 2)
    fig, ax = canvas(880, height)
    title = "Runtime by workload"
    label(ax, 24, 32, title, 19, bold=True)
    label(
        ax,
        24,
        61,
        "Median elapsed time in milliseconds. Lower is faster.",
        10.4,
        color=MUTED,
    )
    for i, (case, medians) in enumerate(rows):
        x, y = 24 + i % 2 * 435, 95 + i // 2 * 137
        ax.hlines(y - 11, x, x + 395, color=RULE, linewidth=0.7)
        label(
            ax, x, y + 8, PANEL_TITLES.get(case["name"], case["name"]), 11.1, bold=True
        )
        label(ax, x, y + 28, case["input"], 8.6, color=MUTED)
        longest = max(medians.values())
        for j, (engine, name) in enumerate(ENGINES.items()):
            bar_y = y + 53 + 22 * j
            ms = medians[engine]
            highlight = engine == "relar"
            ink = "#8c3058" if highlight else INK
            label(ax, x, bar_y, name, 9.5, bold=highlight, color=ink)
            ax.barh(
                bar_y,
                241 * ms / longest,
                left=x + 67,
                height=10,
                color="#bb4b78" if highlight else "#899097",
                linewidth=0,
            )
            label(
                ax,
                x + 395,
                bar_y,
                f"{ms:.2f} ms",
                9.5,
                bold=highlight,
                color=ink,
                align="right",
            )
    ax.hlines(height - 37, 24, 854, color=RULE, linewidth=0.7)
    runs = len(rows[0][0]["samples_ns"])
    label(
        ax,
        24,
        height - 18,
        f"{runs} runs per runtime  ·  Each panel has its own zero-based scale.",
        8.7,
        color=MUTED,
    )
    save(fig, path, title)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("results", type=Path)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    cases = json.loads(args.results.read_bytes())["cases"]
    args.output_dir.mkdir(parents=True, exist_ok=True)
    mpl.rcParams.update(
        {
            "font.family": "DejaVu Sans",
            "svg.fonttype": "none",
            "svg.hashsalt": "relar-runtime-suite",
        }
    )
    times = [
        "| Workload | Input per process | Rust (ms) | CPython (ms) | Relar (ms) | CPython / Relar |",
        "| --- | --- | ---: | ---: | ---: | ---: |",
    ]
    spreads = [
        "| Workload | Rust (ms) | CPython (ms) | Relar (ms) | Output file (bytes) |",
        "| --- | ---: | ---: | ---: | ---: |",
    ]
    rows = []
    for case in cases:
        medians, intervals = {}, []
        for engine in ("rust", "cpython", "relar"):
            samples = [r[engine] / 1e6 for r in case["samples_ns"]]
            low, mid, high = statistics.quantiles(samples, n=4, method="inclusive")
            medians[engine] = mid
            intervals.append(f"{low:.2f}–{high:.2f}")
        cells = " | ".join(f"{mid:.2f}" for mid in medians.values())
        speedup = medians["cpython"] / medians["relar"]
        times.append(f"| {case['name']} | {case['input']} | {cells} | {speedup:.2f}× |")
        spreads.append(
            f"| {case['name']} | {' | '.join(intervals)} | {case['report_bytes']:,} |"
        )
        rows.append((case, medians))

    case, medians = next(
        row for row in rows if row[0]["package"] == "conformance_report"
    )
    conformance_chart(case, medians, args.output_dir / "runtime-conformance.svg")
    workload_panels(rows, args.output_dir / "runtime-suite.svg")
    print("\n".join(times), "\n\n", "\n".join(spreads), sep="")


if __name__ == "__main__":
    main()
