> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Ruff ecosystem report performance

Imre took **0.94–0.96× Rust’s time** to generate Ruff’s frozen Bokeh, Superset and Zulip preview reports. All five frozen reports matched the original CPython output byte-for-byte. The unchanged `ruff-ecosystem --force-preview check` command compared Ruff 0.15.0 and 0.16.1 on 58 projects and reproduced CPython’s 76,952-byte Markdown report. [Upstream Python files and revision](../../examples/ruff_ecosystem/UPSTREAM.json) · [Samples, output hashes and build fingerprints](2026-10-24-ruff-ecosystem-storage.json.gz)

## Frozen reports

Each cell gives **wall time in milliseconds / peak RSS in MiB**. The last column compares wall time. Imre #309 comes from the previous measurement on the same inputs. [Previous measurements](2026-10-24-ruff-ecosystem-preview.md)

| Input | CPython | Imre #309 | Imre | Codon | Rust | Imre / Rust |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Bokeh preview | 171,401 / 182.9 | 760 / 138.2 | 519 / 112.4 | 1,131 / 142.8 | 546 / 63.4 | 0.95× |
| Superset preview | 1,479,775 / 410.7 | 1,986 / 331.6 | 1,370 / 267.3 | 3,799 / 647.0 | 1,433 / 151.6 | 0.96× |
| Zulip preview | 152,859 / 182.2 | 687 / 128.1 | 464 / 104.4 | 1,244 / 236.0 | 492 / 58.8 | 0.94× |
| Sphinx + Prefect, 1× | 2,494 / 153.6 | 380 / 95.3 | 399 / 95.3 | 1,543 / 169.8 | 282 / 52.0 | 1.42× |
| Sphinx + Prefect, 10× | 22,401 / 534.7 | 3,021 / 514.6 | 3,028 / 514.8 | 13,504 / 318.2 | 2,377 / 60.5 | 1.27× |

Bokeh, Superset and Zulip each used one warmup and six interleaved runs per native executable on Linux CPU 3. Their CPython 3.14.5 times come from one previously verified run per project. The standard Sphinx and Prefect cases used four interleaved runs per engine after one warmup, including CPython 3.12.13. Rust implements the replay independently. Codon 0.20.4 compiles Ruff’s Markdown functions with typed adapters. [Capture and replay commands](../../examples/ruff_ecosystem/README.md)

The 10× standard run keeps all 20 project comparisons alive until rendering. Imre used **514.8 MiB** and Rust used **60.5 MiB**. In a sampled heap profile of #309, strings produced by splitting the inputs accounted for 403 MiB, and diff snapshot pointer arrays accounted for another 56 MiB.

## Full 58-project command

Three interleaved runs per Imre build included Git, both Ruff children for each project, and Markdown rendering. Median wall time was **30.09 s on #309 and 29.65 s on this build**. Their respective ranges were 24.17–32.47 s and 21.53–36.32 s. Median peak RSS was **825.1 → 684.6 MiB**. All 58 checkouts remained clean.

The earlier CPython 3.14.5 run took 1,950.32 s and 841.1 MiB. Its 76,952-byte report matched an individual current Imre run (31.37 s, 682.4 MiB) and the earlier #309 run (25.51 s). Both Ruff versions failed on Apache Airflow. Depending on which failure the reporter surfaces, a run can instead produce 76,980 bytes. A current run with that size matched the independently rendered CPython reference for the newer Ruff error (28.53 s, 675.9 MiB). Each report counted 436,327 added and 393,847 removed violations across 47 changed projects, 4 unchanged projects, and 7 errors. [Live benchmark](../ruff_ecosystem_live.py)

## What changed

Each row below compares one change with #309 independently. Values are median milliseconds before → after.

| Change | Bokeh | Superset | Zulip |
| --- | ---: | ---: | ---: |
| Compile literal-delimited regex captures | 741 → 566 | 1,950 → 1,485 | 677 → 531 |
| Reuse immutable strings after stable diff sorting | 764 → 700 | 2,009 → 1,856 | 679 → 642 |
| Replace temporary frozen dataclass copies with scalar values | 740 → 707 | 1,955 → 1,880 | 664 → 647 |

Against the immediate parent, #325, the storage and compiler changes together reduced preview wall time by 9.9–10.2% in six alternating pairs on CPU 23. Superset went from 1,482 to 1,331 ms and from 331.6 to 267.1 MiB.

On 2,048 Bokeh lines per side, retaining original strings reduced the prefix filter’s live bytes at peak from 719,823 to 65,536. Scalar replacement separately reduced allocation events from 92,733 to 84,541. In an independent 388,000-reading field-access loop it reduced 27.26 ms to 2.57 ms. The version that stored the temporary in a list took 34.16 → 33.75 ms. A trial of binary searches for sparse sorted changes increased the 10× time from 3.03 to 3.14 s and was discarded.

Rebuilding both timed programs from the final compiler source reproduced their LLVM and ELF machine-code sections byte-for-byte. [LLVM, executable and machine-code SHA-256 hashes](2026-10-24-ruff-ecosystem-storage.json.gz)
