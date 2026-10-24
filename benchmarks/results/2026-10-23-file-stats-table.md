> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Ruff’s per-file report table

Imre compiles the 81-line `render_file_stats_table` from Ruff at `de907154`. The report adapter binds `CONFORMANCE_DIR_WITH_README` to the requested suite revision in an enclosing function. A source check removes that single level of indentation and compares the full function, including its local `fmt` definition, to the pinned file. The summary and diagnostic-detail renderers still use the typed port. [Ruff source](https://github.com/astral-sh/ruff/blob/de9071547cfec12f689142a7b8ce98edc7aef909/scripts/conformance.py#L776)

## Captured reports

Each fresh process reads the source files, parses the two ty JSON captures, groups diagnostics, computes statistics, renders the full report, and writes it to disk. The 146-file input compares ty 0.0.60 and 0.0.90 on the pinned Python typing suite. The 1,460-file input materializes ten copies with distinct paths and remaps the diagnostics. Unchanged cases use the same ty version on both sides. All timings below include the forced summary table.

Milliseconds are medians of 12 runs per engine and case, after one warmup. All six engines ran on CPU 8 of the same AMD EPYC-Milan x86-64 Linux host in balanced order. Both Imre versions and the Rust reference use jemalloc. CPython 3.14.5 ran pinned Ruff and, separately, the typed port. Native Codon 0.20.4 used the project's existing adapters. For the nested formatter, its adapter removes the keyword-only separator. [Codon build](../codon_native/build.py) · [Matched Rust allocator](2026-10-23-diagnostic-allocator.md)

| Build | Files | Report | Ruff on CPython | Typed CPython | Prior Imre port | Codon | Imre | Rust | Imre / Rust |
| --- | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Ordinary | 146 | changed GitHub | 168.8 | 274.1 | 38.4 | 72.9 | 38.4 | 27.5 | 1.40× |
| Ordinary | 146 | changed diff | 170.1 | 277.2 | 38.1 | 73.8 | 38.8 | 27.1 | 1.43× |
| Ordinary | 146 | unchanged GitHub | 171.6 | 275.5 | 37.9 | 71.1 | 37.5 | 26.0 | 1.44× |
| Ordinary | 146 | unchanged diff | 170.9 | 283.5 | 37.8 | 71.8 | 37.6 | 26.1 | 1.44× |
| Ordinary | 1,460 | changed GitHub | 1006.3 | 2004.3 | 338.0 | 680.7 | 334.3 | 247.2 | 1.35× |
| Ordinary | 1,460 | changed diff | 1000.6 | 2042.0 | 336.6 | 686.9 | 330.8 | 241.4 | 1.37× |
| Ordinary | 1,460 | unchanged GitHub | 948.9 | 2003.8 | 326.4 | 651.4 | 327.3 | 237.0 | 1.38× |
| Ordinary | 1,460 | unchanged diff | 946.9 | 1960.8 | 325.5 | 648.9 | 323.4 | 238.5 | 1.36× |
| ThinLTO | 146 | changed GitHub | 166.0 | 268.0 | 34.3 | 71.6 | 34.4 | 25.4 | 1.35× |
| ThinLTO | 146 | changed diff | 168.1 | 270.9 | 34.4 | 71.9 | 34.4 | 25.2 | 1.37× |
| ThinLTO | 146 | unchanged GitHub | 167.0 | 275.1 | 33.9 | 71.7 | 34.7 | 24.9 | 1.39× |
| ThinLTO | 146 | unchanged diff | 170.3 | 277.6 | 33.9 | 70.7 | 34.5 | 25.4 | 1.36× |
| ThinLTO | 1,460 | changed GitHub | 993.3 | 2029.4 | 302.7 | 693.4 | 304.4 | 230.1 | 1.32× |
| ThinLTO | 1,460 | changed diff | 993.7 | 2022.8 | 303.8 | 690.8 | 303.5 | 228.9 | 1.33× |
| ThinLTO | 1,460 | unchanged GitHub | 972.5 | 2002.7 | 291.0 | 660.3 | 299.4 | 223.4 | 1.34× |
| ThinLTO | 1,460 | unchanged diff | 956.9 | 1983.1 | 292.7 | 661.2 | 295.2 | 227.0 | 1.30× |

All eight captured reports matched upstream byte-for-byte on every run in both link modes. The changed GitHub reports contain 109,258 and 1,101,967 bytes. An independent build of the verbatim, top-level Ruff definition also matched CPython on all six status labels, equal sort keys, and singular and empty output. A caller alternated two revision URLs in the same process and verified the complete table each time. The changed GitHub reports also passed ASan and LSan at both sizes. The application validator passed 367 process checks and 400 seeded report comparisons.

## Live checkout

With both real ty processes included, the changed GitHub report on the 146-file checkout took **671 ms in Imre and 649 ms in Rust** (1.03×). Ruff on CPython took 736 ms and native Codon took 781 ms. All four published the same 109,258 bytes. Each engine ran eight times in balanced order using ordinary optimized builds and two CPU cores.

## Reproduce

Follow the example instructions to materialize the fixtures. Omit `--scales 1` to generate both sizes. The benchmark driver accepts `--reports-only` with one `report` command per engine. [Run the example](../../examples/conformance_report/README.md#replay-a-captured-comparison) · [Benchmark driver](../conformance_grouping_benchmark.py)

The compressed JSON contains both configurations under `captured.normal.config` and `captured.thinlto.config`, all process timings, build commands, input and executable hashes, live arguments, and output digests. The allocator study supplies the runtime and Rust build recipes. [Measurement record](2026-10-23-file-stats-table.json.gz) · [Allocator study](2026-10-23-diagnostic-allocator.md)
