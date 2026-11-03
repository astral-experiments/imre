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
    unavailable=(),
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
        label(ax, x, row_y, name, 14, color=MUTED)
        if engine in unavailable:
            label(ax, x + width, row_y, "unsupported", 13, color=MUTED, align="right")
            continue
        value = medians[engine]
        color = ACCENT if engine == "imre" else BAR
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


def workload_panels(rows, path, *, applications=False):
    row_count = (len(rows) + 1) // 2
    panel_top = 144
    row_height = 180 + 29 * (max(len(medians) for _, medians in rows) - 3)
    height = panel_top + row_height * row_count + 27
    counts = sorted({len(case["samples_ns"]) for case, _ in rows})
    runs = str(counts[0]) if len(counts) == 1 else f"{counts[0]}–{counts[-1]}"
    fig, ax = benchmark_card(
        height,
        "Comparing Imre, Rust, and CPython",
        f"{runs} runs per engine",
    )
    for i, (case, medians) in enumerate(rows):
        x, y = 48 + i % 2 * 423, panel_top + i // 2 * row_height
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
        if applications:
            engines = ENGINES.copy()
            if "original_cpython" in medians:
                engines.update(
                    cpython="CPython · typed", original_cpython="CPython · uv"
                )
            bars(
                ax,
                medians,
                x + 4,
                y + 74,
                393,
                engines=engines,
                label_width=116,
                unavailable=case.get("unavailable", ()),
            )
        else:
            bars(ax, medians, x + 4, y + 74, 393)
    save(fig, path, "Median runtime by workload")


def frozen_flask_panel(report, path):
    engines = {
        "imre": "Frozen Flask on Imre",
        "rust": "Rust / Hyper",
    }
    if any(
        row["server"] == "cpython" and row["profile"] == "mixed"
        for row in report["trials"]
    ):
        engines["cpython"] = "Stock Flask / CPython"
    settings = report["settings"]
    concurrency = settings["concurrency"]
    first, rounds = settings["first_round"], settings["rounds"]
    expected_rounds = list(range(first, first + rounds))
    runs = [report, *report.get("additional_runs", ())]
    medians = {}
    for engine in engines:
        rates = []
        for run in runs:
            trials = [
                row
                for row in run["trials"]
                if row["server"] == engine
                and row["profile"] == "mixed"
                and row["concurrency"] == concurrency
            ]
            if sorted(row["round"] for row in trials) != expected_rounds:
                raise ValueError(f"Incomplete mixed HTTP trials for {engine}")
            rates.extend(row["RPS"] for row in trials)
        medians[engine] = statistics.median(rates)

    fig, ax = benchmark_card(
        408 + 29 * (len(engines) - 4),
        "Frozen Flask throughput",
        f"{rounds * len(runs)} rounds · {concurrency} clients",
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
    report = json.loads(args.results.read_bytes())
    if not report.get("complete", True):
        parser.error("the measurement is incomplete")
    cases = report["cases"]
    applications = report.get("suite") == "applications" or "runner_sha256" in report
    args.output_dir.mkdir(parents=True, exist_ok=True)
    mpl.rcParams.update(
        {
            "font.family": "sans-serif",
            "font.sans-serif": ["DejaVu Sans"],
            "svg.fonttype": "none",
            "svg.hashsalt": "imre-runtime-suite",
        }
    )
    engines = {"rust": "Rust", "cpython": "CPython", "imre": "Imre"}
    if applications:
        engines = {
            "rust": "Rust",
            "imre": "Imre",
            "cpython": "CPython",
            "original_cpython": "CPython (original uv)",
        }
    title = " | ".join(f"{name} (ms)" for name in engines.values())
    right = " | ".join(["---:"] * len(engines))
    times = [
        f"| Workload | Input per process | {title} | CPython / Imre |",
        f"| --- | --- | {right} | ---: |",
    ]
    spreads = [f"| Workload | {title} | Output (bytes) |", f"| --- | {right} | ---: |"]
    rows = []
    for case in cases:
        medians, values, intervals = {}, [], []
        for engine in engines:
            if engine not in case["samples_ns"][0]:
                values.append("—")
                intervals.append("—")
                continue
            samples = [r[engine] / 1e6 for r in case["samples_ns"]]
            low, mid, high = statistics.quantiles(samples, n=4, method="inclusive")
            medians[engine] = mid
            values.append(f"{mid:.2f}")
            intervals.append(f"{low:.2f}–{high:.2f}")
        cells = " | ".join(values)
        speedup = (
            f"{medians['cpython'] / medians['imre']:.2f}×" if "imre" in medians else "—"
        )
        times.append(f"| {case['name']} | {case['input']} | {cells} | {speedup} |")
        spreads.append(
            f"| {case['name']} | {' | '.join(intervals)} | {case['report_bytes']:,} |"
        )
        rows.append((case, medians))
    filename = "applications.svg" if applications else "runtime-suite.svg"
    workload_panels(rows, args.output_dir / filename, applications=applications)
    if args.flask_results:
        opener = gzip.open if args.flask_results.suffix == ".gz" else open
        with opener(args.flask_results, "rt") as stream:
            frozen_flask_panel(json.load(stream), args.output_dir / "frozen-flask.svg")
    print("\n".join(times), "\n\n", "\n".join(spreads), sep="")


if __name__ == "__main__":
    main()
