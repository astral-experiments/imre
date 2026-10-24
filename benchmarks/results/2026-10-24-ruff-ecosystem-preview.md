> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Ruff ecosystem preview comparisons

Ruff’s unchanged `ruff-ecosystem --force-preview check` took **25.51 seconds with Imre and 32 minutes 30 seconds with CPython 3.14.5** on 58 pinned checkouts. Both compared Ruff 0.15.0 against 0.16.1. Their 76,952-byte Markdown reports matched byte-for-byte, with 436,327 added and 393,847 removed violations across 47 changed projects, four unchanged projects and seven project errors. [Upstream source](../../examples/ruff_ecosystem/UPSTREAM.json) · [Samples, hashes and all 58 revisions](2026-10-24-ruff-ecosystem-preview.json.gz)

| Full CLI, one run per engine | Wall time | Peak RSS |
| --- | ---: | ---: |
| CPython 3.14.5 | 1,950.322 s | 841.1 MiB |
| Imre | 25.512 s | 827.0 MiB |

The 76.45× comparison includes Git, both Ruff processes for each project, and Markdown rendering. Both engines used the same checkout paths and identical wrappers to capture Ruff’s stdout, stderr and exit status. Each Ruff child used one CPU, with up to 16 CPUs available across children. Every checkout was clean at its recorded revision after the run. The earlier Imre executable had produced no report after 650 seconds. The earlier CPython 3.12 run reached its 500-second timeout.

Both Ruff versions failed on Apache Airflow. In this pair the reporter surfaced the older Ruff error. A separate Imre run surfaced the newer Ruff error and produced 76,980 bytes. The timing of parallel child failures affects which error appears, and some errors include the absolute checkout path. The matching pair’s SHA-256 is `c6123d930aa27778287d77d149229db95c247c15e358d7362ad27073dac32d0c`.

## Frozen diagnostics

Each process reads captured Ruff output, filters and sorts the diagnostics, computes their diff, and renders Markdown. The three preview projects exercise the change from rule codes to rule names. The two standard cases replay Sphinx and Prefect without preview. At 10× the harness gives each project ten distinct names. All five reports matched the original CPython output byte-for-byte. [Preview fixture](../fixtures/ruff_ecosystem/preview.json) · [Standard fixture](../fixtures/ruff_ecosystem/corpus.json)

| Input | Diagnostic lines across both sides | Report bytes | CPython | Imre | Codon 0.20.4 | Rust | Imre / Rust |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Bokeh preview | 152,749 | 42,221 | 171.401 s | 0.760 s | 1.133 s | 0.525 s | 1.45× |
| Superset preview | 359,860 | 38,840 | 1,479.775 s | 1.986 s | 3.843 s | 1.417 s | 1.40× |
| Zulip preview | 142,781 | 26,666 | 152.859 s | 0.687 s | 1.265 s | 0.489 s | 1.41× |
| Sphinx and Prefect, 1× | 502,684 | 14,693 | 2.403 s | 0.380 s | 1.486 s | 0.270 s | 1.41× |
| Sphinx and Prefect, 10× | 5,026,840 | 22,848 | 22.343 s | 3.021 s | 13.194 s | 2.360 s | 1.28× |

| Peak RSS | CPython | Imre | Codon | Rust |
| --- | ---: | ---: | ---: | ---: |
| Bokeh preview | 182.9 MiB | 138.2 MiB | 142.9 MiB | 63.4 MiB |
| Superset preview | 410.7 MiB | 331.6 MiB | 646.1 MiB | 151.7 MiB |
| Zulip preview | 182.2 MiB | 128.1 MiB | 236.5 MiB | 58.8 MiB |
| Sphinx and Prefect, 1× | 152.3 MiB | 95.3 MiB | 170.0 MiB | 52.0 MiB |
| Sphinx and Prefect, 10× | 534.6 MiB | 514.6 MiB | 319.1 MiB | 60.5 MiB |

The preview times for Imre, Codon and Rust are medians of six interleaved runs after one warmup on Linux CPU 3. CPython 3.14.5 ran each full preview once on separate CPUs, Bokeh on 28, Superset on 29 and Zulip on 27. The standard cases used four interleaved runs per engine on CPU 3 after one warmup, with CPython 3.12.13. Rust implements the replay independently. Codon compiles Ruff’s Markdown functions with typed adapters. Their comparisons cover frozen replay. Ruff’s original program keeps all 20 comparison results alive until it renders the 10× report.

## CPU and allocations

On the first 192 sorted Bokeh diagnostics per side, `ndiff` accounted for 92.5% of the prior Imre executable’s instructions. With the final compiler and runtime, the complete replay used **19× fewer instructions**. Both profiles produced the same 15,202 report bytes as CPython 3.12.

| Bokeh, 192 diagnostics per side | Prior Imre | Final Imre |
| --- | ---: | ---: |
| Callgrind instructions | 409,828,309 | 21,559,262 |
| DHAT allocation events | 150,349 | 13,868 |
| Total bytes allocated | 33,590,753 | 4,142,523 |
| Peak live heap bytes | 676,162 | 613,246 |

Across the 51 successful projects, CPython 3.12’s line matcher produced 53 replacement blocks. No block had a line in common between its two sides. Superset’s largest block alone contained 32.3 billion possible line pairs for intraline matching. The reporter discards intraline hints and sorts signed diagnostics by their text. Imre can defer the full diff for that access pattern and index repeated membership checks on the added and removed string lists.

After those changes, building dictionaries of named regex captures still cost 13.8% of instructions on the 2,048-diagnostic Bokeh prefix. The final build reads the proven capture keys directly. Instructions fell from 207.5 million to 154.4 million on that prefix, with 92,732 allocation events versus 132,066. CPython 3.12.13 and 3.14.5 also matched on all seven prefixes. At 2,048 per side they took 108.30 seconds and 1.36 seconds respectively and emitted the same 19,167 bytes.

The capture helper regenerates the preview inputs from pinned checkouts. The `preview` benchmark takes their parent directory and accepts `--python` to rerun the full CPython reference. Use the live benchmark’s `--force-preview` option for the complete CLI. [Capture and replay commands](../../examples/ruff_ecosystem/README.md) · [Benchmark harness](../ruff_ecosystem_benchmark.py) · [Timing samples and profiler counters](2026-10-24-ruff-ecosystem-preview.json.gz)
