> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Ruff’s memory report

Imre and CPython run Ruff’s original `scripts/memory_report.py`. The `compare` measurements include process startup, directory discovery, JSON decoding, ranking, Markdown rendering, and writing the full report to stdout. Codon runs the native typed port, and the independent Rust implementation uses Serde and native integers with a checked arbitrary-precision fallback. Imre and Rust both use ThinLTO and the system allocator. [Ruff source](https://github.com/astral-sh/ruff/blob/de9071547cfec12f689142a7b8ce98edc7aef909/scripts/memory_report.py) · [Measurement record](2026-10-23-original-memory-report.json.gz)

| Comparison | Projects | Items across old and new | CPython (ms) | Codon (ms) | Imre (ms) | Rust (ms) | Imre / Rust |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1× changed | 2 | 844 | 75.47 | 12.80 | 5.44 | 3.37 | 1.61× |
| 1× unchanged | 2 | 844 | 75.98 | 12.25 | 5.09 | 3.23 | 1.57× |
| 10× projects changed | 20 | 8,440 | 93.57 | 25.33 | 19.70 | 11.93 | 1.65× |
| 10× projects unchanged | 20 | 8,440 | 91.94 | 22.51 | 17.31 | 11.48 | 1.51× |
| 10× rows changed | 2 | 8,440 | 94.55 | 22.59 | 18.02 | 11.18 | 1.61× |
| 10× rows unchanged | 2 | 8,440 | 92.13 | 20.33 | 15.61 | 10.19 | 1.53× |

The 1× input contains the unmodified `ty` 0.0.89 and 0.0.90 reports for Ruff’s scripts and Imre’s inventory example. The 10× project layout repeats the same files under distinct project names. The 10× row layout keeps two projects, gives every item copy a distinct name, and scales the top-line memory counters. The unchanged cases compare the old captures against themselves and produce a 51-byte report. [Capture provenance](../../examples/memory_report/sample/provenance.json)

Imre enforces Ruff’s declared record types when decoding JSON. For example, an absent `structs` field receives Ruff’s empty-list default, while an explicit `null` raises `TypeError`. CPython can finish an unchanged report without reading that invalid list. All timed captures satisfy the declared types.

Each median covers 24 measured executions after a warmup on one AMD EPYC Milan CPU. A four-engine rotation balances execution positions and neighbors. All 576 runs completed successfully with empty stderr. The 1× and 10× project reports matched CPython byte for byte, as did the unchanged 10× row reports. In the synthetic changed-row layout, copying each memory change ten times creates ties. Ruff sorts the members of a set by the absolute change and displays only its top 15. The validator checks each observed row and count, its change magnitude at each rank, and every byte outside the tied rows. The source, inputs, executables, output digests, and individual timings are recorded. [Benchmark](../memory_program_benchmark.py) · [Comparison oracle](../memory_validate.py)

The upstream `run` subcommand also prepares the projects and executes both `ty` binaries. Those wall-clock timings are reported separately. [Live project runs](2026-10-23-original-memory-live.md)

## Reproduce

Build the system ThinLTO runtime and select an available CPU. The Codon reference uses the official 0.20.4 Linux x86-64 release. The Rust reference builder accepts `--toolchain ohm` on a development machine. [ThinLTO build instructions](../../docs/subset.md#build-and-validate) · [Native Codon comparison](../codon_native/README.md#memory-report-comparisons)

```sh
"$IMRE" build "$RUFF/scripts/memory_report.py" --whole-program --opt-level 3 \
  --runtime "$THIN_RUNTIME" --target x86_64-unknown-linux-gnu \
  --lto-linker "$LTO_LINKER" -o "$BUILD/memory-report"
"$CPYTHON" benchmarks/reference/memory_report/build.py --thinlto \
  --artifact-dir "$BUILD/rust"
"$CPYTHON" benchmarks/codon_native/build.py --codon "$CODON" \
  --applications memory_report --output "$BUILD/codon"
"$CPYTHON" tests/ruff_memory_compare.py --source "$RUFF/scripts/memory_report.py" \
  --python "$CPYTHON" --binary "$BUILD/memory-report"
"$CPYTHON" benchmarks/memory_program_benchmark.py --source "$RUFF/scripts/memory_report.py" \
  --python "$CPYTHON" --imre "$BUILD/memory-report" --compiler "$IMRE" --runtime "$THIN_RUNTIME" \
  --rust "$BUILD/rust/memory-report-reference" --codon "$BUILD/codon/memory_report" \
  --cpu 3 --samples 24 --output measurements.json.gz
```
