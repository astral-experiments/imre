# Rust, CPython and Imre

Median elapsed time in milliseconds, including startup, imports and file I/O. Lower is faster. The [README](../README.md#benchmarks) lists all six medians for each runtime.

![Six panels comparing median elapsed time for Rust, Imre and CPython on inventory reconciliation, Base45 decoding, Luhn checksums, telemetry and offline conformance reporting.](results/runtime-suite.svg)

The ranges below are the 25th and 75th percentiles of the same 12 measurements on a shared VM. The last column gives the size of the file each implementation had to reproduce byte for byte.

| Workload | Rust (ms) | CPython (ms) | Imre (ms) | Output file (bytes) |
| --- | ---: | ---: | ---: | ---: |
| Inventory (repeated) | 26.82–37.70 | 84.60–100.20 | 42.94–57.25 | 654,942 |
| Inventory (distinct) | 48.79–58.75 | 120.13–145.75 | 78.00–103.53 | 1,438,871 |
| Base45 decode | 9.71–11.23 | 206.75–256.81 | 19.34–20.42 | 16,384 |
| Luhn checksum | 5.39–7.95 | 182.00–251.94 | 16.33–19.90 | 1 |
| Telemetry report | 10.02–20.68 | 139.94–178.11 | 18.48–24.90 | 123 |
| Conformance report | 30.81–43.56 | 310.83–358.85 | 50.56–60.35 | 109,258 |

## What each process does

- **Inventory.** Reconcile 16,384 positive requests against either 64 or 16,384 stock-keeping units, then write the complete fulfillment report. Both fixtures also contain zero and negative quantities. The distinct-product fixture includes unknown products. The Rust port preserves insertion order and arbitrary-precision quantities. [Inventory reference](reference/inventory_reconciliation/)
- **Base45.** Decode a 24 KiB ASCII payload to 16 KiB of binary data 64 times. Each call contributes to a checksum, and the process writes the last decoded buffer. Python runs the vendored codec unchanged. Rust decodes into a preallocated buffer. [Base45 reference](reference/base45_codec/)
- **Luhn.** Compute the checksum of the same 16-digit string 65,536 times. The upstream Python implementation builds a digit list and sums alternating slices. Rust streams the digits. Both drivers write the final checksum and print an accumulator covering every call. [Luhn reference](reference/luhn_checksum/)
- **Telemetry.** Load two timestamped traces once and run 81 complete reports on 4,096 aligned pairs. The input mixes positive and negative measurement errors. The report includes aggregate errors and threshold crossings, and stdout checks the repeated work. [Telemetry reference](reference/telemetry_comparison/)
- **Conformance.** Read 146 typing-suite files and the captured JSON from ty 0.0.60 and 0.0.90, group diagnostics and render the GitHub report. CPython and Imre both run the typed offline port. CPython uses its Python compatibility module to decode typed JSON records. The unmodified Ruff renderer supplies the expected bytes. The original CLI has a separate comparison that includes running ty. [Offline reference](reference/conformance_report/) · [Original CLI measurements](results/2026-10-23-original-conformance.md)

## How we measured

We timed each command from process creation through exit on one logical CPU of an AMD EPYC Milan Linux x86-64 VM. Times include interpreter startup, imports, parsing, computation, file I/O and output. Fixture generation, compilation and validation run outside the timer. For every process the runner clears the previous output, checks the exit status and stderr, and compares stdout and the complete output file with the expected bytes.

Each workload gets two warmups and twelve measured runs per implementation. The measured rounds use all six engine orders twice. The filesystem cache is warm. The README's speedup divides the CPython median by the Imre median. The [raw samples](results/runtime-suite.json) include the engine order within each round, versions, binary hashes and output hashes.

The October 28, 2026 run used CPython 3.14.5 with PGO, LTO, the GIL enabled and the JIT disabled. Imre 0.1.3 compiled at optimization level 3 with Clang 18.1.3 and the system allocator. The Rust references and Imre runtime used rustc 1.98.1-dev (Ohm), release optimization level 3, 16 codegen units and LTO disabled. We passed `-Zohm-defaults=no` to Cargo because Ohm’s experimental defaults triggered a compiler error while building the runtime.

## Reproduce

Run the suite from the repository root on Linux with CPython 3.14+, Clang 18 and the repository's Rust toolchain installed.

```sh
python3.14 benchmarks/runtime_suite.py --output benchmarks/local/runtime-suite.json
```

The runner builds the native executables before timing and selects the first permitted CPU. Pass `--cpu N` to select another one. The recorded run used `--cpu 30 --toolchain ohm`. This command writes the six workload panels used in the README and this guide (`runtime-suite.svg`) and prints both Markdown tables from the committed data.

```sh
uv run --script benchmarks/render_runtime_suite.py benchmarks/results/runtime-suite.json \
  --output-dir benchmarks/local
```
