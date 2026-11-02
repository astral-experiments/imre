# /// script
# requires-python = ">=3.11"
# dependencies = ["matplotlib==3.10.8"]
# ///
"""Render the benchmark charts and print the README and benchmark-guide tables."""

import argparse
import gzip
import json
import statistics
from pathlib import Path

import matplotlib as mpl
import matplotlib.pyplot as plt
from matplotlib.patches import FancyBboxPatch, Rectangle

PANEL_TITLES = {
    "Inventory (repeated)": "Inventory · repeated",
    "Inventory (distinct)": "Inventory · distinct",
    "Conformance report": "Offline conformance report",
}
# Match the benchmark cards in astral-experiments/serc.
SURFACE, BORDER = "#261230", "#503d60"
INK, MUTED = "#fbf2fc", "#c4b6d3"
BAR, ACCENT = "#cdcbfb", "#de5fe9"
WIDTH = 840


def label(ax, x, y, text, size, *, color=INK, align="left", mono=False):
    ax.text(
        x,
        y,
        text,
        fontsize=size * 72 / 96,
        fontfamily="DejaVu Sans Mono" if mono else "DejaVu Sans",
        color=color,
        va="center",
        ha=align,
    )


def benchmark_card(height, title, subtitle):
    fig = plt.figure(figsize=(WIDTH / 96, height / 96), dpi=96)
    ax = fig.add_axes((0, 0, 1, 1), xlim=(0, WIDTH), ylim=(height, 0))
    ax.set_axis_off()
    ax.add_patch(
        FancyBboxPatch(
            (9, 15),
            WIDTH - 18,
            height - 28,
            boxstyle="round,pad=0,rounding_size=19",
            facecolor=SURFACE,
            edgecolor=BORDER,
            linewidth=0.8,
            zorder=-1,
        )
    )
    label(ax, 42, 54, title, 34)
    label(ax, 43, 88, subtitle, 17, color=MUTED)
    return fig, ax


def bar(ax, x, y, width, color):
    ax.add_patch(Rectangle((x, y - 4), width, 8, facecolor=color, edgecolor="none"))


def save(fig, path, title):
    fig.savefig(
        path,
        format="svg",
        transparent=True,
        metadata={"Date": None, "Title": title},
    )
    plt.close(fig)
    # Matplotlib leaves trailing spaces at line breaks inside SVG paths.
    svg = path.read_text().replace(" \n", "\n")
    # Keep text selectable and use local font fallbacks in GitHub's SVG images.
    svg = svg.replace(
        "'DejaVu Sans Mono'",
        "ui-monospace, 'SFMono-Regular', Consolas, 'Liberation Mono', monospace",
    )
    path.write_text(svg.replace("'DejaVu Sans'", "Arial, Helvetica, sans-serif"))


def workload_chart(rows, path):
    row_height = 39
    height = 184 + len(rows) * row_height
    fig, ax = benchmark_card(
        height, "Runtime by workload", "Median elapsed time in milliseconds"
    )

    time_columns = (
        ("rust", "Rust", 316),
        ("imre", "Imre", 407),
        ("cpython", "CPython", 512),
    )
    label(ax, 43, 136, "Workload", 15, color=MUTED)
    for _, name, x in time_columns:
        label(ax, x, 136, name, 15, color=MUTED, align="right")
    label(ax, 558, 136, "Imre speedup over CPython", 15, color=MUTED)
    ax.plot((43, 797), (152, 152), color=BORDER, linewidth=0.6)

    max_speedup = max(speedup for _, _, speedup in rows) or 1
    for i, (case, medians, speedup) in enumerate(rows):
        y = 177 + i * row_height
        label(ax, 43, y, PANEL_TITLES.get(case["name"], case["name"]), 17)
        for engine, _, x in time_columns:
            label(ax, x, y, f"{medians[engine]:.2f}", 17, align="right", mono=True)
        bar(ax, 558, y, 169 * speedup / max_speedup, ACCENT)
        label(ax, 797, y, f"{speedup:.1f}×", 17, align="right", mono=True)
    save(fig, path, "Median runtime and Imre speedup over CPython by workload")


def frozen_flask_panel(report, path):
    engines = {
        "imre": "Frozen Flask on Imre",
        "node": "Node.js",
        "bun": "Bun",
        "rust": "Rust / Hyper",
    }
    settings = report["settings"]
    concurrency = settings["concurrency"]
    first, rounds = settings["first_round"], settings["rounds"]
    expected_rounds = list(range(first, first + rounds))
    medians = {}
    for engine in engines:
        trials = [
            row
            for row in report["trials"]
            if row["server"] == engine
            and row["profile"] == "mixed"
            and row["concurrency"] == concurrency
        ]
        if sorted(row["round"] for row in trials) != expected_rounds:
            raise ValueError(f"Incomplete mixed HTTP trials for {engine}")
        medians[engine] = statistics.median(row["RPS"] for row in trials)

    fig, ax = benchmark_card(
        279,
        "Frozen Flask throughput",
        f"Median requests per second with {concurrency} clients",
    )
    fastest = max(medians.values()) or 1
    for i, engine in enumerate(sorted(engines, key=medians.__getitem__, reverse=True)):
        y = 133 + i * 35
        label(ax, 43, y, engines[engine], 17)
        bar(
            ax,
            225,
            y,
            476 * medians[engine] / fastest,
            ACCENT if engine == "imre" else BAR,
        )
        label(ax, 797, y, f"{medians[engine]:,.0f}", 17, align="right", mono=True)
    save(fig, path, "Frozen Flask mixed HTTP throughput")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("results", type=Path)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument(
        "--flask-results", type=Path, help="also chart a mixed HTTP comparison receipt"
    )
    args = parser.parse_args()
    cases = json.loads(args.results.read_bytes())["cases"]
    args.output_dir.mkdir(parents=True, exist_ok=True)
    mpl.rcParams.update(
        {
            "font.family": "sans-serif",
            "font.sans-serif": ["DejaVu Sans"],
            "svg.fonttype": "none",
            "svg.hashsalt": "imre-runtime-suite",
        }
    )
    times = [
        "| Workload | Input per process | Rust (ms) | CPython (ms) | Imre (ms) | CPython / Imre |",
        "| --- | --- | ---: | ---: | ---: | ---: |",
    ]
    spreads = [
        "| Workload | Rust (ms) | CPython (ms) | Imre (ms) | Output file (bytes) |",
        "| --- | ---: | ---: | ---: | ---: |",
    ]
    rows = []
    for case in cases:
        medians, intervals = {}, []
        for engine in ("rust", "cpython", "imre"):
            samples = [r[engine] / 1e6 for r in case["samples_ns"]]
            low, mid, high = statistics.quantiles(samples, n=4, method="inclusive")
            medians[engine] = mid
            intervals.append(f"{low:.2f}–{high:.2f}")
        cells = " | ".join(f"{mid:.2f}" for mid in medians.values())
        speedup = medians["cpython"] / medians["imre"]
        times.append(f"| {case['name']} | {case['input']} | {cells} | {speedup:.2f}× |")
        spreads.append(
            f"| {case['name']} | {' | '.join(intervals)} | {case['report_bytes']:,} |"
        )
        rows.append((case, medians, speedup))

    workload_chart(rows, args.output_dir / "runtime-suite.svg")
    if args.flask_results:
        opener = gzip.open if args.flask_results.suffix == ".gz" else open
        with opener(args.flask_results, "rt") as stream:
            frozen_flask_panel(json.load(stream), args.output_dir / "frozen-flask.svg")
    print("\n".join(times), "\n\n", "\n".join(spreads), sep="")


if __name__ == "__main__":
    main()
