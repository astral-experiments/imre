> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Compressed diff snapshots in Ruff’s ecosystem checker

On the 10× Sphinx and Prefect replay, peak RSS fell from **249.6 to 123.9 MiB** (50.4% lower). Median wall time was **2.77 → 2.71 s** in six paired runs against #332. All five frozen Markdown reports matched CPython byte-for-byte. [Previous measurements](2026-10-24-ruff-ecosystem-packed.md) · [Raw samples, build hashes and input revisions](2026-10-24-ruff-ecosystem-diff-blocks.json.gz)

## Frozen reports

Each cell gives **wall time in milliseconds / peak RSS in MiB**. The last column compares wall time.

| Input | CPython | Codon | Rust | Imre | Imre / Rust |
| --- | ---: | ---: | ---: | ---: | ---: |
| Bokeh preview | 171,401 / 182.9 | 1,114 / 142.8 | 527 / 63.4 | 508 / 112.4 | 0.97× |
| Superset preview | 1,479,775 / 410.7 | 3,843 / 646.0 | 1,372 / 151.7 | 1,347 / 267.1 | 0.98× |
| Zulip preview | 152,859 / 182.2 | 1,239 / 236.5 | 479 / 58.8 | 458 / 104.6 | 0.96× |
| Sphinx + Prefect, 1× | 2,417 / 153.5 | 1,474 / 170.0 | 280 / 52.5 | 375 / 83.9 | 1.34× |
| Sphinx + Prefect, 10× | 22,326 / 534.4 | 13,347 / 300.4 | 2,373 / 60.5 | 2,636 / 123.8 | 1.11× |

The 10× workload keeps 20 project comparisons alive until rendering. The standard cases used one warmup and four interleaved measurements per engine on Linux CPU 3, including a fresh CPython 3.12.13 run. The previews used one warmup and six measured rounds per native engine. The CPython 3.14.5 preview times come from earlier runs of the unchanged script on these exact captures. Codon 0.20.4 uses typed adapters, and Rust implements the replay independently. [Capture and replay commands](../../examples/ruff_ecosystem/README.md)

## Recovery and storage

When the report selects a few changes from large sorted inputs, Imre compares eight prefix bytes at a time and groups the common-prefix lengths and remaining row bytes into 64 KiB blocks. It keeps a block compressed when LZ4 reduces its size. Incompressible blocks and oversized rows stay raw. A full diff can reconstruct the original inputs with one reusable block buffer.

In a separate twelve-round experiment on the 10× corpus, the previous prefix encoder, word-sized prefix comparisons, and word-sized comparisons with LZ4 took **2.614 → 2.577 → 2.552 s**. Peak RSS was **249.6 → 249.6 → 123.8 MiB**.

When Ruff’s original `Diff.from_pair(...).lines` read the complete diff, Sphinx took **147 → 144 ms** and Prefect took **396 → 385 ms** across twelve paired runs. Prefect’s peak RSS fell from **89.5 to 82.7 MiB**. A direct `ndiff` control on Prefect took 386 → 391 ms when reading the full diff first and 439 → 449 ms after selecting changes. The complete diffs matched CPython 3.12 and 3.14, including their intraline guides. Six more cases covered duplicates, Unicode, oversized rows, and shared prefixes across block boundaries. They matched both interpreters across 12,447,267 output bytes.

## Full 58-project command

The unchanged `ruff-ecosystem --force-preview check` command compared Ruff 0.15.0 and 0.16.1 on 58 pinned checkouts. In three interleaved runs per build, median wall time was **25.28 → 23.71 s** and peak RSS was **683.9 → 684.4 MiB**. The time ranges overlapped and include Git and both Ruff children per project. One of three candidate runs reproduced the original 76,952-byte CPython 3.14 report. The other two runs surfaced the newer Ruff failure on Apache Airflow and matched its 76,980-byte Python reference. All checkouts remained clean. [Live benchmark](../ruff_ecosystem_live.py)
