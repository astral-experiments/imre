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
# Galaxy, Lunar and Radiate: https://astral.sh
# Light-mode purple: https://github.com/astral-sh/uv/blob/main/assets/svg/Benchmark-Light.svg
INK, MUTED = "#261230", "#6f5d6f"
RULE, SURFACE = "#e2d5e8", "#fbf2fc"
BAR, ACCENT = "#c9c1f4", "#6340ac"
PALETTE = {
    "ink": (INK, "#f7eff8"),
    "muted": (MUTED, "#b8a5c3"),
    "rule": (RULE, "#493554"),
    "surface": (SURFACE, "#261230"),
    "bar": (BAR, "#c9c1f4"),
    "accent": (ACCENT, "#d7ff64"),
}


def canvas(width, height):
    fig = plt.figure(figsize=(width / 96, height / 96), dpi=96)
    ax = fig.add_axes((0, 0, 1, 1), xlim=(0, width), ylim=(height, 0))
    ax.set_axis_off()
    return fig, ax


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


def surface(ax, width, height):
    ax.add_patch(
        FancyBboxPatch(
            (0, 0),
            width,
            height,
            boxstyle="round,pad=0,rounding_size=12",
            facecolor=SURFACE,
            edgecolor="none",
            zorder=-1,
        )
    )


def rule(ax, x0, y0, x1, y1):
    ax.plot((x0, x1), (y0, y1), color=RULE, linewidth=0.6, zorder=0)


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
    svg = svg.replace("'DejaVu Sans'", "Arial, Helvetica, sans-serif")
    for name, (light, _) in PALETTE.items():
        svg = svg.replace(light, f"var(--{name})")
    light = "; ".join(f"--{name}: {colors[0]}" for name, colors in PALETTE.items())
    dark = "; ".join(f"--{name}: {colors[1]}" for name, colors in PALETTE.items())
    style = f"""<style>
    :root {{{light}}}
    text {{
      font-variant-numeric: tabular-nums;
    }}
    @media (prefers-color-scheme: dark) {{:root {{{dark}}}}}
  </style>"""
    path.write_text(svg.replace("<defs>", f"<defs>\n  {style}", 1))


def bars(
    ax, medians, x, y, width, *, engines=ENGINES, label_width=68, value_format=".2f"
):
    value_width = 60
    row_step, bar_height = 40, 14
    font_size = 13
    plot_x = x + label_width
    plot_width = width - label_width - value_width
    ticks = MaxNLocator(nbins=3, steps=(1, 2, 2.5, 5, 10), integer=True).tick_values(
        0, max(medians.values())
    )
    last_y = y + row_step * (len(engines) - 1)
    for tick in ticks:
        tick_x = plot_x + plot_width * tick / ticks[-1]
        rule(ax, tick_x, y - 20, tick_x, last_y + 20)
        label(
            ax,
            tick_x,
            last_y + 36,
            f"{tick:g}",
            11,
            color=MUTED,
            align="center",
            mono=True,
        )
    for i, (engine, name) in enumerate(engines.items()):
        row_y = y + row_step * i
        value = medians[engine]
        highlight = engine == "imre"
        color = ACCENT if highlight else INK
        bar_width = plot_width * value / ticks[-1]
        label(ax, x, row_y, name, font_size, color=color)
        ax.add_patch(
            Rectangle(
                (plot_x, row_y - bar_height / 2),
                bar_width,
                bar_height,
                facecolor=ACCENT if highlight else BAR,
                edgecolor="none",
                zorder=2,
            )
        )
        label(
            ax,
            plot_x + bar_width + 9,
            row_y,
            format(value, value_format),
            font_size,
            color=color,
            mono=True,
        )


def workload_panels(rows, path):
    row_count = (len(rows) + 1) // 2
    height = 148 + 252 * row_count + 36
    fig, ax = canvas(800, height)
    surface(ax, 800, height)
    label(ax, 32, 30, "IMRE / RUNTIME BENCHMARKS", 11, color=MUTED, mono=True)
    label(ax, 32, 68, "RUNTIME BY WORKLOAD", 26)
    label(ax, 32, 98, "Rust, CPython and Imre on six workloads.", 14, color=MUTED)
    rule(ax, 32, 122, 768, 122)
    for i, (case, medians) in enumerate(rows):
        x, y = 32 + i % 2 * 384, 158 + i // 2 * 252
        label(ax, x, y, PANEL_TITLES.get(case["name"], case["name"]), 20)
        label(ax, x, y + 28, case["input"], 12, color=MUTED)
        bars(ax, medians, x, y + 75, 352)
        if i % 2 == 0 and i // 2 < row_count - 1:
            rule(ax, 32, y + 216, 768, y + 216)
    runs = len(rows[0][0]["samples_ns"])
    label(
        ax,
        32,
        height - 26,
        f"Median elapsed time (ms) · {runs} runs · Lower is faster · Independent scales",
        12,
        color=MUTED,
    )
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

    fig, ax = canvas(800, 352)
    surface(ax, 800, 352)
    label(ax, 32, 30, "IMRE / HTTP BENCHMARKS", 11, color=MUTED, mono=True)
    label(ax, 32, 68, "FROZEN FLASK THROUGHPUT", 26)
    label(
        ax,
        32,
        98,
        "Health checks, escaped HTML greetings and JSON sums.",
        14,
        color=MUTED,
    )
    rule(ax, 32, 122, 768, 122)
    bars(
        ax, medians, 32, 155, 736, engines=engines, label_width=148, value_format=",.0f"
    )
    label(
        ax,
        32,
        330,
        f"Median requests/s · {rounds} rounds of {settings['requests']:,} requests · "
        f"{concurrency} clients · Higher is faster",
        12,
        color=MUTED,
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
