# Rust, CPython and Imre

Median elapsed time in milliseconds, including startup, imports and file I/O. Lower is faster. The [README](../README.md#benchmark) lists all six medians for each runtime.

![Six panels comparing median elapsed time for Rust, Imre and CPython on inventory reconciliation, Base45 decoding, Luhn checksums, telemetry and offline conformance reporting.](results/runtime-suite.svg)

The ranges below are the 25th and 75th percentiles of the same 12 measurements. The last column gives the size of the file each implementation had to reproduce byte for byte.

| Workload | Rust (ms) | CPython (ms) | Imre (ms) | Output file (bytes) |
| --- | ---: | ---: | ---: | ---: |
| Inventory (repeated) | 24.93–25.67 | 75.63–79.18 | 29.38–31.37 | 654,942 |
| Inventory (distinct) | 45.46–46.45 | 110.43–116.17 | 52.54–53.72 | 1,438,871 |
| Base45 decode | 8.92–9.78 | 182.71–187.73 | 15.04–15.71 | 16,384 |
| Luhn checksum | 5.03–5.54 | 148.51–150.76 | 12.82–13.79 | 1 |
| Telemetry report | 9.16–9.78 | 119.05–124.40 | 13.10–13.94 | 123 |
| Conformance report | 23.35–24.07 | 245.37–254.15 | 32.65–33.55 | 109,258 |

## What each process does

- **Inventory.** Reconcile 16,384 positive requests against either 64 or 16,384 stock-keeping units, then write the complete fulfillment report. Both fixtures also contain zero and negative quantities. The distinct-product fixture includes unknown products. The Rust port preserves insertion order and arbitrary-precision quantities. [Inventory reference](reference/inventory_reconciliation/)
- **Base45.** Decode a 24 KiB ASCII payload to 16 KiB of binary data 64 times. Each call contributes to a checksum, and the process writes the last decoded buffer. Python runs the vendored codec unchanged. Rust decodes into a preallocated buffer. [Base45 reference](reference/base45_codec/)
- **Luhn.** Compute the checksum of the same 16-digit string 65,536 times. The upstream Python implementation builds a digit list and sums alternating slices. Rust streams the digits. Both drivers write the final checksum and print an accumulator covering every call. [Luhn reference](reference/luhn_checksum/)
- **Telemetry.** Load two timestamped traces once and run 81 complete reports on 4,096 aligned pairs. The input mixes positive and negative measurement errors. The report includes aggregate errors and threshold crossings, and stdout checks the repeated work. [Telemetry reference](reference/telemetry_comparison/)
- **Conformance.** Read 146 typing-suite files and the captured JSON from ty 0.0.60 and 0.0.90, group diagnostics and render the GitHub report. CPython and Imre both run the typed offline port. CPython uses its Python compatibility module to decode typed JSON records. The unmodified Ruff renderer supplies the expected bytes. The original CLI has a separate comparison that includes running ty. [Offline reference](reference/conformance_report/) · [Original CLI measurements](results/2026-10-23-original-conformance.md)

## How we measured

We timed each command from process creation through exit on one logical CPU of an AMD EPYC Milan Linux x86-64 VM. Times include interpreter startup, imports, parsing, computation, file I/O and output. Fixture generation, compilation and validation run outside the timer. For every process the runner clears the previous output, checks the exit status and stderr, and compares stdout and the complete output file with the expected bytes.

Each workload gets two warmups and twelve measured runs per implementation. The measured rounds use all six engine orders twice. The filesystem cache is warm. The README's speedup divides the CPython median by the Imre median. The [raw samples](results/runtime-suite.json) include the engine order within each round, versions, binary hashes and output hashes.

The October 23, 2026 run used CPython 3.14.5 with PGO, LTO, the GIL enabled and the JIT disabled. Imre 0.1.1 compiled at optimization level 3 with Clang 18.1.3 and the system allocator. The Rust references and Imre runtime used rustc 1.98.1-dev (Ohm), release optimization level 3, 16 codegen units and LTO disabled.

## Reproduce

Run the suite from the repository root on Linux with CPython 3.14+, Clang 18 and the repository's Rust toolchain installed.

```sh
python3.14 benchmarks/runtime_suite.py --output benchmarks/local/runtime-suite.json
```

The runner builds the native executables before timing and selects the first permitted CPU. Pass `--cpu N` to select another one. The recorded run used `--cpu 12 --toolchain ohm`. This command writes the six workload panels used in the README and this guide (`runtime-suite.svg`), a standalone conformance chart (`runtime-conformance.svg`), and both Markdown tables from the committed data.

```sh
uv run --script benchmarks/render_runtime_suite.py benchmarks/results/runtime-suite.json \
  --output-dir benchmarks/local
```
