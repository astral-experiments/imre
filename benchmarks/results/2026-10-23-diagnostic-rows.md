# Ruff’s diagnostic row renderers

Relar compiles Ruff’s `render_diff_row`, `render_html_diff_row`, `TyDiagnostic.__str__`, and `TyDiagnostic.severity_for_display` unchanged. The GitHub report links each diagnostic group to its minimum and maximum line in the selected checkout. The diff report prints the diagnostic’s path, line, column, severity, rule, and description. [Pinned Ruff row renderers](https://github.com/astral-sh/ruff/blob/de9071547cfec12f689142a7b8ce98edc7aef909/scripts/conformance.py#L353-L412) · [Diagnostic display](https://github.com/astral-sh/ruff/blob/de9071547cfec12f689142a7b8ce98edc7aef909/scripts/conformance.py#L208-L246)

## Full reports

The frozen reports took **1.35–1.43× Rust’s time**. The live checkout, including both ty invocations, took **1.01×** for GitHub and **1.02×** for diff. Every warmup and timed run wrote exactly the same bytes as pinned Ruff under CPython.

| Files, comparison, format | CPython (ms) | Codon (ms) | Relar #130 (ms) | Relar (ms) | Rust (ms) | Relar / Rust | Bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 146, changed, GitHub | 171.5 | 73.3 | 37.6 | 38.3 | 27.1 | 1.41× | 109,258 |
| 146, changed, diff | 172.5 | 73.9 | 37.6 | 38.1 | 26.9 | 1.41× | 96,947 |
| 146, unchanged, GitHub | 170.6 | 71.7 | 36.8 | 37.7 | 26.4 | 1.43× | 1,489 |
| 146, unchanged, diff | 170.5 | 73.8 | 38.3 | 37.8 | 27.0 | 1.40× | 1,489 |
| 1,460, changed, GitHub | 984.8 | 686.2 | 354.4 | 355.9 | 262.6 | 1.36× | 1,101,967 |
| 1,460, changed, diff | 979.3 | 694.6 | 357.3 | 362.9 | 261.0 | 1.39× | 979,040 |
| 1,460, unchanged, GitHub | 966.9 | 674.9 | 346.2 | 352.5 | 256.1 | 1.38× | 1,503 |
| 1,460, unchanged, diff | 978.4 | 662.9 | 356.1 | 345.3 | 255.4 | 1.35× | 1,503 |
| 146, live, GitHub | 736.1 | 780.4 | 655.9 | 655.6 | 646.6 | 1.01× | 109,258 |
| 146, live, diff | 728.7 | 781.7 | 661.5 | 651.9 | 642.1 | 1.02× | 98,897 |

Times are medians in milliseconds from ten fresh processes per case and engine after one warmup. The run order balanced positions and neighboring engines. Frozen runs used one CPU on an AMD EPYC-Milan Linux host and included startup, reading inputs, computing the full report, and writing the output. The 10× fixture copies the suite ten times with distinct filenames and diagnostic locations. The live runs used two CPUs and included discovery, ty 0.0.60 and 0.0.90, and atomic publication. Live diff reports print checkout paths that differ from the captures.

Both Relar executables use the default module-by-module build and the same current system-allocator runtime. The #130 compiler reproduced all seven application objects from its original benchmark byte for byte. Rust uses its independent release build and the system allocator. CPython 3.14.5 runs the upstream source through the existing input adapter. Codon 0.20.4 uses its native generator extrema and record display. Its adapter passes the checkout URLs to the nested helper and uses `str.format_map` with a string filename and an integer line. [Codon adapter](../codon_native/README.md#live-conformance-workflow)

The first full pass had intermittent slowdowns across all five engines during the 10× changed diff case. We repeated the entire matrix after local builds finished and a malformed-format-string exception was corrected. The seven current application objects were byte-identical across the two builds. The table uses the complete second run. The record retains both runs, including warmups, execution order, inputs, output hashes, executable identities, and comparator builds. [Raw measurements](2026-10-23-diagnostic-rows.json.gz)

## Output checks

The source check compares all four definitions to pinned Ruff, including the property decorator. A seeded differential compared 400 reports from the Python port with Ruff. A direct row check covers empty groups, single lines, ranges, all three severity labels, Unicode paths and messages, and both signs. We also checked a Unicode checkout revision, repeated named fields and escaped braces, and an unknown field that leaves the destination intact. Relar matched Ruff on CPython and native Codon for those revisions. The independent Rust reference treats braces in a revision literally. Its timed inputs use a pinned hexadecimal revision. [Source check](../../tests/classes.py)

The final instrumented compiler output and Rust runtime also passed AddressSanitizer and LeakSanitizer on all eight frozen reports, both live formats, and the Unicode revision. A focused formatter differential matched CPython on 2,380 valid templates and 396 malformed or competing-error cases.

## Reproduce

Prepare both fixture scales with `--scales 1 10`, build the Codon and Rust references, and compile the current reporter and the #130 source with their respective Relar compilers. Use the current `librelar_runtime.a` for both. [Fixture preparation](../../examples/conformance_report/README.md#replay-a-captured-comparison) · [Codon builder](../codon_native/build.py) · [Rust builder](../reference/conformance_report/build.py)

```sh
python3 benchmarks/conformance_render_benchmark.py --full-matrix \
  --fixtures "$FIXTURES" --python "$CPYTHON" --previous "$PREVIOUS" \
  --relar "$RELAR_REPORT" --codon "$CODON_REPORT" --rust "$RUST_REPORT" --cpu 8 \
  --live "$TYPING/conformance" "$OLD_TY" "$NEW_TY" --live-cpus 20 21 \
  --output measurements.json.gz
```
