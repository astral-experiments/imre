# /// script
# requires-python = ">=3.11"
# dependencies = ["matplotlib==3.10.8"]
# ///
"""Render the workload panels and print the README and benchmark-guide tables."""

import argparse
import gzip
import json
import statistics
from pathlib import Path

import matplotlib as mpl
import matplotlib.pyplot as plt
from matplotlib.patches import FancyBboxPatch, Rectangle
from matplotlib.ticker import MaxNLocator

ENGINES = {"rust": "Rust", "imre": "Imre", "cpython": "CPython"}
PANEL_TITLES = {
    "Inventory (repeated)": "Inventory · repeated",
    "Inventory (distinct)": "Inventory · distinct",
    "Conformance report": "Offline conformance report",
}
# Match the benchmark cards in astral-experiments/serc.
SURFACE, BORDER, RULE = "#261230", "#503d60", "#453250"
INK, MUTED = "#fbf2fc", "#c4b6d3"
BAR, ACCENT = "#cdcbfb", "#de5fe9"
WIDTH = 920


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


def benchmark_card(height, title, metadata):
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
            linewidth=1.2,
            zorder=-1,
        )
    )
    ax.add_patch(
        FancyBboxPatch(
            (48, 41),
            31,
            31,
            boxstyle="round,pad=0,rounding_size=7",
            facecolor=BAR,
            edgecolor="none",
        )
    )
    for i, width in enumerate((17, 12, 7)):
        ax.add_patch(Rectangle((55, 49 + 5 * i), width, 3, color=SURFACE, linewidth=0))
    label(ax, 91, 57, "IMRE / BENCHMARKS", 13, color=MUTED, mono=True)
    label(ax, 872, 57, metadata, 13, color=MUTED, align="right", mono=True)
    label(ax, 46, 114, title, 32)
    return fig, ax


def rule(ax, x0, y0, x1, y1):
    ax.plot((x0, x1), (y0, y1), color=RULE, linewidth=0.8, zorder=0)


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


def bars(
    ax,
    medians,
    x,
    y,
    width,
    *,
    engines=ENGINES,
    label_width=83,
    value_format="{:.2f} ms",
):
    row_step, bar_height = 29, 10
    plot_x = x + label_width
    plot_width = width - label_width - 99
    ticks = MaxNLocator(nbins=3, steps=(1, 2, 2.5, 5, 10), integer=True).tick_values(
        0, max(medians.values())
    )
    last_y = y + row_step * (len(engines) - 1)
    for tick in ticks:
        tick_x = plot_x + plot_width * tick / ticks[-1]
        rule(ax, tick_x, y - 6, tick_x, last_y + 6)
        label(
            ax,
            tick_x,
            last_y + 24,
            f"{tick:,g}",
            12,
            color=MUTED,
            align="center",
            mono=True,
        )
    for i, (engine, name) in enumerate(engines.items()):
        row_y = y + row_step * i
        value = medians[engine]
        color = ACCENT if engine == "imre" else BAR
        label(ax, x, row_y, name, 14, color=MUTED)
        ax.add_patch(
            Rectangle(
                (plot_x, row_y - bar_height / 2),
                plot_width * value / ticks[-1],
                bar_height,
                facecolor=color,
                edgecolor="none",
                zorder=2,
            )
        )
        label(
            ax,
            x + width,
            row_y,
            value_format.format(value),
            15,
            color=color,
            align="right",
            mono=True,
        )


def workload_panels(rows, path):
    row_count = (len(rows) + 1) // 2
    panel_top = 144
    height = panel_top + 180 * row_count + 27
    runs = len(rows[0][0]["samples_ns"])
    fig, ax = benchmark_card(
        height,
        "Comparing Imre, Rust, and CPython",
        f"{runs} runs per engine",
    )
    for i, (case, medians) in enumerate(rows):
        x, y = 48 + i % 2 * 423, panel_top + i // 2 * 180
        if i >= 2:
            rule(ax, x, y, x + 401, y)
        label(
            ax,
            x + 4,
            y + 23,
            PANEL_TITLES.get(case["name"], case["name"]),
            17,
        )
        label(ax, x + 4, y + 47, case["input"], 13, color=MUTED)
        bars(ax, medians, x + 4, y + 74, 393)
    save(fig, path, "Median runtime by workload")


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
        408,
        "Frozen Flask throughput",
        f"{rounds} rounds · {concurrency} clients",
    )
    label(
        ax,
        48,
        158,
        f"{settings['requests']:,} requests per round, split across health checks, HTML greetings and JSON sums.",
        16,
        color=MUTED,
    )
    rule(ax, 48, 192, 872, 192)
    label(ax, 52, 217, "Median requests per second", 17)
    label(ax, 868, 217, "Higher is faster", 13, color=MUTED, align="right")
    bars(
        ax,
        medians,
        52,
        251,
        816,
        engines=engines,
        label_width=180,
        value_format="{:,.0f}",
    )
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
        rows.append((case, medians))

    workload_panels(rows, args.output_dir / "runtime-suite.svg")
    if args.flask_results:
        opener = gzip.open if args.flask_results.suffix == ".gz" else open
        with opener(args.flask_results, "rt") as stream:
            frozen_flask_panel(json.load(stream), args.output_dir / "frozen-flask.svg")
    print("\n".join(times), "\n\n", "\n".join(spreads), sep="")


if __name__ == "__main__":
    main()
