> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Ruff ecosystem diagnostic replay

Two releases of Ruff checked the complete Sphinx and Prefect trees with all rules selected. Imre runs Ruff’s original report functions over 502,684 captured diagnostic lines at 1×. The 10× input repeats each project under ten distinct names for 5,026,840 lines. Each timed process reads the captures, filters and sorts diagnostics, computes the diff and renders Markdown.

| Input | CPython 3.12.13 | Imre | Codon 0.20.4 | Rust | Imre / Rust |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1× | 2.447 s | 0.507 s | 1.490 s | 0.273 s | 1.86× |
| 10× | 22.422 s | 4.119 s | 13.408 s | 2.404 s | 1.71× |

| Peak RSS | CPython | Imre | Codon | Rust |
| --- | ---: | ---: | ---: | ---: |
| 1× | 153.5 MiB | 109.6 MiB | 169.9 MiB | 52.0 MiB |
| 10× | 534.6 MiB | 507.1 MiB | 355.0 MiB | 60.5 MiB |

The two Markdown reports matched CPython byte-for-byte, including GitHub truncation at 10×. Ruff retains each full line diff through rendering. The Rust reference retains the changed diagnostics it needs to render the same output.

Times and peak RSS are medians of four measured runs per implementation after one warmup. Each process ran pinned to CPU 3 on x86-64 Linux, with a rotated running order. Rust is an independent implementation. Codon uses Ruff’s Markdown functions with typed adapters and a native diff implementation.

[Capture and replay commands](../../examples/ruff_ecosystem/README.md) · [Input and output hashes](../fixtures/ruff_ecosystem/corpus.json) · [Samples and executable hashes](2026-10-24-ruff-ecosystem.json.gz)
