# Ruff’s conformance summary

Relar compiles Ruff’s `render_summary`, `diff_format`, and `Statistics` unchanged. The summary uses the checkout revision supplied to the reporter. When both ty versions agree, Ruff prints a compact summary by default. Passing `--force-summary-table` also prints the metrics table. When the new analyzer has no true positives, the assertion prints the first five grouped cases. [Ruff’s summary and metric formatting](https://github.com/astral-sh/ruff/blob/de9071547cfec12f689142a7b8ce98edc7aef909/scripts/conformance.py#L859-L1010) · [Ruff’s statistics](https://github.com/astral-sh/ruff/blob/de9071547cfec12f689142a7b8ce98edc7aef909/scripts/conformance.py#L415-L433)

## Full reports

Relar took **1.34–1.43× Rust’s time on frozen reports** and **1.01–1.03× on live checkouts**, including both ty processes. All 880 warmup and timed runs produced the same bytes as pinned Ruff on CPython.

| Files and workload | CPython (ms) | Codon (ms) | Relar #132 (ms) | Relar (ms) | Rust (ms) | Relar / Rust | Bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 146, changed, GitHub | 163.8 | 73.0 | 38.1 | 38.6 | 27.0 | 1.43× | 109,258 |
| 146, changed, diff | 161.9 | 72.5 | 37.0 | 37.6 | 26.8 | 1.40× | 96,947 |
| 146, unchanged table, GitHub | 161.2 | 71.8 | 36.9 | 37.0 | 26.4 | 1.40× | 1,489 |
| 146, unchanged table, diff | 157.8 | 70.9 | 36.5 | 36.7 | 26.1 | 1.40× | 1,489 |
| 146, unchanged compact, GitHub | 160.7 | 73.3 | 36.9 | 37.2 | 26.5 | 1.40× | 469 |
| 146, unchanged compact, diff | 161.4 | 72.5 | 36.9 | 38.3 | 27.2 | 1.41× | 469 |
| 1,460, changed, GitHub | 961.5 | 681.5 | 360.0 | 360.0 | 263.7 | 1.37× | 1,101,967 |
| 1,460, changed, diff | 969.7 | 691.3 | 353.2 | 363.6 | 260.9 | 1.39× | 979,040 |
| 1,460, unchanged table, GitHub | 950.5 | 660.9 | 341.7 | 341.9 | 252.2 | 1.36× | 1,503 |
| 1,460, unchanged table, diff | 938.9 | 657.5 | 345.3 | 345.7 | 254.8 | 1.36× | 1,503 |
| 1,460, unchanged compact, GitHub | 990.0 | 659.7 | 357.1 | 352.2 | 257.5 | 1.37× | 471 |
| 1,460, unchanged compact, diff | 961.1 | 659.3 | 345.4 | 344.3 | 257.0 | 1.34× | 471 |
| 146, live changed, GitHub | 738.7 | 775.7 | 664.4 | 657.0 | 649.4 | 1.01× | 109,258 |
| 146, live changed, diff | 719.4 | 774.7 | 652.9 | 662.3 | 644.5 | 1.03× | 98,897 |
| 146, live compact, GitHub | 709.4 | 776.3 | 647.4 | 643.7 | 628.2 | 1.02× | 469 |
| 146, live compact, diff | 706.2 | 764.8 | 644.0 | 645.5 | 626.9 | 1.03× | 469 |

The 1× capture contains 146 files. The 10× capture repeats them with distinct filenames and diagnostic locations. Changed captures compare ty 0.0.60 with 0.0.90. Unchanged captures compare identical analyzer outputs. Live changed cases run both ty versions from the real checkout. Live compact cases run ty 0.0.90 twice. Diff uses real checkout paths in live mode, which accounts for its different byte count.

The table gives medians of ten fresh processes per engine and workload after one warmup. The run order balances positions and neighboring engines. Times include startup, filesystem reads, report computation, and output. Frozen runs used one CPU and live runs used two on the same AMD EPYC-Milan Linux host. Both Relar reporters use the same compiler built on top of #134, the default seven-module build, and the same system-allocator runtime. Compiling the #132 Python port with its earlier compiler reproduced all seven application objects and the complete linked executable byte for byte.

CPython 3.14.5 runs the pinned script through the repository’s input adapter. Native Codon 0.20.4 uses the shared summary body with its own dedent implementation and Python-style record representations. The independent Rust release build uses its system allocator. [Codon adapters](../codon_native/README.md#live-conformance-workflow) · [Rust reference](../reference/conformance_report/src/report.rs)

## Output checks

The unforced, unchanged report is 469 bytes at 1× and 471 bytes at 10×. Its forced counterpart is 1,489 and 1,503 bytes respectively. We compared both formats and both flag settings against Ruff on changed and unchanged captures. We also checked the full and compact forms with a multiline revision whose interpolation changes the common indent. The independent Rust reference matched Ruff on 156 report outputs across ordinary and multiline inputs. Codon’s dedent matched CPython on 192 direct inputs, including all 29 Python whitespace code points and differing tab and space prefixes.

The zero-true-positive fixture contains seven groups. Ruff’s assertion displays the first five in order, with the enclosing lists, nested dataclasses, Unicode paths, quoted descriptions and enum members. Relar, Codon and Rust matched the full exception text, 3,064 bytes at the recorded paths including the exception name and final newline, and preserved the existing output file. The previous port omitted the sampled records. The validator checks the complete message separately from interpreter traceback decorations and rejects an extra line or a truncated record. ASan/LSan and ThinLTO each passed 18 full-output and error checks. [Conformance validator](../conformance_validate.py) · [Source fidelity check](../../tests/classes.py)

## Reproduce

Prepare captures with `--scales 1 10`. Build both the current reporter and the #132 Python port with the current Relar compiler, linking the same system runtime into both. Build the Codon and Rust references, then run the comparison below. The measurement record retains inputs, executable hashes, commands, balanced execution order, every timing sample, output hashes and comparator build identities. [Fixture preparation](../../examples/conformance_report/README.md#replay-a-captured-comparison) · [Native Codon builder](../codon_native/build.py) · [Rust builder](../reference/conformance_report/build.py) · [Measurement record](2026-10-23-conformance-summary.json.gz)

```sh
"$CPYTHON" benchmarks/conformance_render_benchmark.py --full-matrix --compact \
  --fixtures "$FIXTURES" --python "$CPYTHON" --previous "$PREVIOUS" \
  --relar "$RELAR_REPORT" --codon "$CODON_REPORT" --rust "$RUST_REPORT" --cpu 8 \
  --live "$TYPING/conformance" "$OLD_TY" "$NEW_TY" --live-cpus 20 21 \
  --output measurements.json.gz
```
