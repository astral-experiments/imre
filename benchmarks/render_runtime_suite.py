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
from matplotlib.patches import FancyBboxPatch

ENGINES = {"rust": "Rust", "relar": "Relar", "cpython": "CPython"}
PANEL_TITLES = {
    "Inventory (repeated)": "Inventory · repeated",
    "Inventory (distinct)": "Inventory · distinct",
    "Conformance report": "Offline conformance report",
}
INK, MUTED = "#192b28", "#667772"
RULE, SURFACE = "#dee7e2", "#ffffff"
BAR, TRACK, ACCENT = "#b5c2be", "#eef2f0", "#127b67"
PALETTE = {
    "ink": (INK, "#e5eeeb"),
    "muted": (MUTED, "#9eaea8"),
    "rule": (RULE, "#2c3934"),
    "surface": (SURFACE, "#111a16"),
    "bar": (BAR, "#667b71"),
    "track": (TRACK, "#233029"),
    "accent": (ACCENT, "#64d3b2"),
}


def canvas(width, height):
    fig = plt.figure(figsize=(width / 96, height / 96), dpi=96)
    ax = fig.add_axes((0, 0, 1, 1), xlim=(0, width), ylim=(height, 0))
    ax.set_axis_off()
    return fig, ax


def label(ax, x, y, text, size, *, weight="normal", color=INK, align="left"):
    ax.text(
        x,
        y,
        text,
        fontsize=size * 72 / 96,
        weight=weight,
        color=color,
        va="center",
        ha=align,
    )


def box(ax, x, y, width, height, color, *, radius=4, border="none"):
    ax.add_patch(
        FancyBboxPatch(
            (x, y),
            width,
            height,
            boxstyle=f"round,pad=0,rounding_size={min(radius, width / 2)}",
            facecolor=color,
            edgecolor=border,
            linewidth=0.75,
        )
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
    style = f"""<style>
    :root {{{light}}}
    text {{
      font-family: system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif !important;
      font-variant-numeric: tabular-nums;
    }}
    @media (prefers-color-scheme: dark) {{:root {{{dark}}}}}
  </style>"""
    path.write_text(svg.replace("<defs>", f"<defs>\n  {style}", 1))


def conformance_chart(case, medians, path):
    fig, ax = canvas(800, 312)
    box(ax, 0.5, 0.5, 799, 311, SURFACE, radius=14, border=RULE)
    label(ax, 32, 39, "Conformance report", 23, weight="semibold")
    label(ax, 32, 66, case["input"], 14, color=MUTED)
    label(ax, 768, 39, "Median elapsed time", 13, color=MUTED, align="right")
    label(ax, 768, 61, "Lower is faster", 13, color=MUTED, align="right")
    longest = max(medians.values())
    for i, (engine, name) in enumerate(ENGINES.items()):
        y = 112 + 66 * i
        ms = medians[engine]
        highlight = engine == "relar"
        color = ACCENT if highlight else INK
        weight = "semibold" if highlight else "normal"
        label(ax, 32, y, name, 16, weight=weight, color=color)
        label(ax, 768, y, f"{ms:.2f} ms", 19, weight=weight, color=color, align="right")
        box(ax, 32, y + 20, 736, 10, TRACK, radius=5)
        box(
            ax,
            32,
            y + 20,
            736 * ms / longest,
            10,
            ACCENT if highlight else BAR,
            radius=5,
        )
    save(fig, path, "Median time to render the offline conformance report")


def workload_panels(rows, path):
    height = 292 * ((len(rows) + 1) // 2) + 12
    fig, ax = canvas(800, height)
    for i, (case, medians) in enumerate(rows):
        x, y = i % 2 * 412, i // 2 * 292
        box(ax, x + 0.5, y + 0.5, 387, 267, SURFACE, radius=12, border=RULE)
        label(
            ax,
            x + 24,
            y + 34,
            PANEL_TITLES.get(case["name"], case["name"]),
            20,
            weight="semibold",
        )
        label(ax, x + 24, y + 61, case["input"], 13, color=MUTED)
        longest = max(medians.values())
        for j, (engine, name) in enumerate(ENGINES.items()):
            row_y = y + 101 + 56 * j
            ms = medians[engine]
            highlight = engine == "relar"
            ink = ACCENT if highlight else INK
            weight = "semibold" if highlight else "normal"
            label(ax, x + 24, row_y, name, 15, weight=weight, color=ink)
            label(
                ax,
                x + 364,
                row_y,
                f"{ms:.2f} ms",
                17,
                weight=weight,
                color=ink,
                align="right",
            )
            box(ax, x + 24, row_y + 19, 340, 8, TRACK)
            box(
                ax,
                x + 24,
                row_y + 19,
                340 * ms / longest,
                8,
                ACCENT if highlight else BAR,
            )
    runs = len(rows[0][0]["samples_ns"])
    label(
        ax,
        4,
        height - 10,
        f"Median of {runs} runs · Lower is faster · Each panel has its own zero-based scale",
        12,
        color=MUTED,
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
            "font.sans-serif": ["DejaVu Sans"],
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
