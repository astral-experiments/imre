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
from matplotlib.patches import Rectangle
from matplotlib.ticker import MaxNLocator

ENGINES = {"rust": "Rust", "relar": "Relar", "cpython": "CPython"}
PANEL_TITLES = {
    "Inventory (repeated)": "Inventory · repeated products",
    "Inventory (distinct)": "Inventory · distinct products",
    "Conformance report": "Offline conformance report",
}
INK, MUTED, FAINT = "#0a0a0a", "#525252", "#707070"
RULE, SURFACE, BAR = "#e2e2e2", "#ffffff", "#181818"
ACCENT, ACCENT_INK = "#ff1f8f", "#d60066"
PALETTE = {
    "ink": (INK, "#e6edf3"),
    "muted": (MUTED, "#a8b1bb"),
    "faint": (FAINT, "#919ba5"),
    "rule": (RULE, "#30363d"),
    "surface": (SURFACE, "#0d1117"),
    "bar": (BAR, "#969694"),
    "accent": (ACCENT, "#ff2e97"),
    "accent-ink": (ACCENT_INK, "#ff5cb0"),
}


def canvas(width, height):
    fig = plt.figure(figsize=(width / 96, height / 96), dpi=96)
    ax = fig.add_axes((0, 0, 1, 1), xlim=(0, width), ylim=(height, 0))
    ax.set_axis_off()
    return fig, ax


def label(ax, x, y, text, size, *, bold=False, color=INK, align="left", mono=False):
    ax.text(
        x,
        y,
        text,
        fontsize=size * 72 / 96,
        family="monospace" if mono else "sans-serif",
        weight="bold" if bold else "normal",
        color=color,
        va="center",
        ha=align,
    )


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
    for name, (light, _) in PALETTE.items():
        svg = svg.replace(light, f"var(--{name})")
    light = "; ".join(f"--{name}: {colors[0]}" for name, colors in PALETTE.items())
    dark = "; ".join(f"--{name}: {colors[1]}" for name, colors in PALETTE.items())
    style = f"<style>:root {{{light}}} @media (prefers-color-scheme: dark) {{:root {{{dark}}}}}</style>"
    path.write_text(svg.replace("<defs>", f"<defs>\n  {style}", 1))


def conformance_chart(medians, path):
    fig, ax = canvas(800, 156)
    start, width = 104, 600
    ticks = MaxNLocator(nbins=5, steps=[1, 2, 2.5, 5, 10]).tick_values(
        0, max(medians.values())
    )
    for tick in ticks:
        x = start + width * tick / ticks[-1]
        ax.vlines(x, 24, 120, color=RULE, linewidth=0.6, zorder=0)
        label(ax, x, 136, f"{tick:g} ms", 12, color=MUTED, align="center")
    for i, (engine, name) in enumerate(ENGINES.items()):
        y = 38 + 34 * i
        ms = medians[engine]
        length = width * ms / ticks[-1]
        highlight = engine == "relar"
        label(ax, start - 10, y, name, 13, bold=highlight, align="right")
        ax.barh(y, length, left=start, height=18, color="#ffcf00", linewidth=0)
        label(ax, start + length + 8, y, f"{ms:.2f} ms", 13, bold=highlight)
    save(fig, path, "Median time to render the offline conformance report")


def workload_panels(rows, path):
    height = 232 * ((len(rows) + 1) // 2) + 20
    fig, ax = canvas(800, height)
    for i, (case, medians) in enumerate(rows):
        x, y = i % 2 * 408, i // 2 * 232
        ax.add_patch(
            Rectangle(
                (x + 0.5, y + 0.5),
                391,
                215,
                facecolor=SURFACE,
                edgecolor=RULE,
                linewidth=0.75,
            )
        )
        label(
            ax,
            x + 20,
            y + 28,
            PANEL_TITLES.get(case["name"], case["name"]),
            15,
            bold=True,
        )
        label(ax, x + 20, y + 50, case["input"], 12.5, color=FAINT)
        longest = max(medians.values())
        for j, (engine, name) in enumerate(ENGINES.items()):
            bar_y = y + 86 + 32 * j
            ms = medians[engine]
            highlight = engine == "relar"
            ink = INK if highlight else MUTED
            label(ax, x + 20, bar_y, name, 12.5, bold=highlight, color=ink)
            ax.hlines(bar_y + 7, x + 84, x + 288, color=RULE, linewidth=0.6)
            ax.barh(
                bar_y,
                204 * ms / longest,
                left=x + 84,
                height=14,
                color=ACCENT if highlight else BAR,
                linewidth=0,
            )
            label(
                ax,
                x + 372,
                bar_y,
                f"{ms:.2f} ms",
                13,
                bold=highlight,
                color=ink,
                align="right",
                mono=True,
            )
        ax.hlines(y + 174, x + 20, x + 372, color=RULE, linewidth=0.6)
        speedup = medians["cpython"] / medians["relar"]
        label(
            ax,
            x + 20,
            y + 194,
            f"{speedup:.2f}× faster than CPython",
            12,
            color=ACCENT_INK,
            mono=True,
        )
    runs = len(rows[0][0]["samples_ns"])
    label(
        ax,
        0,
        height - 12,
        f"Median of {runs} runs. Lower is faster. Each panel has its own zero-based scale.",
        11.5,
        color=FAINT,
    )
    save(fig, path, "Median runtime by workload")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("results", type=Path)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    cases = json.loads(args.results.read_bytes())["cases"]
    args.output_dir.mkdir(parents=True, exist_ok=True)
    mpl.rcParams.update(
        {
            "font.family": "sans-serif",
            "font.sans-serif": ["Arial", "Liberation Sans", "sans-serif"],
            "font.monospace": [
                "SFMono-Regular",
                "Consolas",
                "Liberation Mono",
                "monospace",
            ],
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

    _, medians = next(row for row in rows if row[0]["package"] == "conformance_report")
    conformance_chart(medians, args.output_dir / "runtime-conformance.svg")
    workload_panels(rows, args.output_dir / "runtime-suite.svg")
    print("\n".join(times), "\n\n", "\n".join(spreads), sep="")


if __name__ == "__main__":
    main()
