# Packed struct lists vs frozen dataclasses

With one million separately constructed two-float points, packed Imre structs built **3.18× faster** and scanned **2.75× faster** than Imre frozen dataclasses. Peak resident memory fell from **56.4 to 18.0 MiB**. These are medians on one machine. [Raw samples, tool hashes and run order](2026-10-25-struct-points.json).

## One million separately constructed points

| Implementation | Build (ms) | 12 scans (ms) | Peak RSS (MiB) |
| --- | ---: | ---: | ---: |
| Imre struct | 29.79 | 11.90 | 18.0 |
| Imre dataclass | 94.80 | 32.70 | 56.4 |
| CPython struct | 758.5 | 1241.9 | 129.8 |
| CPython dataclass | 533.7 | 1243.0 | 128.8 |

The loop appends `Point(x, x + 1.0)` once for each index, cycling through 1,024 coordinate pairs. Every scan calls `p.norm2()` and multiplies by that pass’s scale. Imre writes both coordinates into a 16-byte row per element. The scan reads those rows directly. In the baseline, each independently constructed dataclass has a record allocation and the list holds pointers. On CPython both lists store object references; the struct compatibility decorator additionally validates constructor argument types.

Rebuilding the two programs reproduced both timed LLVM hashes. In the emitted build loop,
the dataclass variant calls `imre_record_try_new`; the struct variant calls
`imre_struct_append` with the scalar payload and has no record allocation.
The struct scan obtains `imre_struct_data_readonly` once before the loop.

## Smaller independently constructed lists

At 10,000 elements, the two native scan times are close relative to the million-point case:

| Implementation | Build (ms) | 12 scans (ms) | Peak RSS (MiB) |
| --- | ---: | ---: | ---: |
| Imre struct | 0.397 | 0.112 | 3.3 |
| Imre dataclass | 0.972 | 0.136 | 3.5 |
| CPython struct | 7.92 | 12.27 | 15.8 |
| CPython dataclass | 4.27 | 12.25 | 15.0 |

## Repeating one existing point

The control builds `[Point(3.0, 4.0)] * 1_000_000`, then runs the same scans:

| Implementation | Build (ms) | 12 scans (ms) | Peak RSS (MiB) |
| --- | ---: | ---: | ---: |
| Imre struct | 12.71 | 11.68 | 18.0 |
| Imre dataclass | 11.42 | 11.58 | 10.5 |
| CPython struct | 6.31 | 1372.5 | 22.3 |
| CPython dataclass | 6.23 | 1357.2 | 21.5 |

Here the dataclass list shares *one* object. Its slots hold 8-byte pointers; the packed list stores a 16-byte value in each slot. The memory advantage reverses: 18.0 MiB for structs versus 10.5 MiB for dataclasses on Imre. Native packed repetition reserves the output once and doubles its initialized prefix. These results do not measure workloads where most retrieved points must be materialized as independent objects.

## Method and reproduction

- Source commit `e95bcefc0180`; [fixture](../fixtures/struct_points.py), [harness](../struct_points.py). The harness produces the baseline by changing only the decorator to `@dataclass(frozen=True, slots=True)` and its import.
- AMD EPYC-Milan Processor, x86_64, Linux, pinned to CPU 30. Python 3.14.5; Ubuntu clang version 18.1.3 (1ubuntu1). Imre: LLVM `-O3 --whole-program`, no ThinLTO, same release system-allocator runtime archive for both executables. This run used rustc 1.99.0-dev (Ohm 1.99.0-7) with Ohm’s experimental defaults disabled.
- Eight measured processes per implementation per workload, after one unmeasured warmup each. A balanced four-way order rotates between rounds. Times are measured inside the program, excluding compilation, startup, printing and teardown. Linux GNU time reports peak RSS for the entire child process, including the runtime or interpreter.
- Every warmup and sample must print the expected length and an exact binary64 checksum independently calculated using integer arithmetic. The raw file includes the checksums, phase timings and per-process peak RSS. Shared-host scheduling and cache state influence the results.

From the repository root, choose an allowed CPU and run outside ptrace:

```sh
cargo +ohm -Zohm-defaults=no build --release --locked -p imre-compiler -p imre-runtime
python3.14 benchmarks/struct_points.py \
  --compiler "${CARGO_TARGET_DIR:-target}/release/imre" \
  --runtime "${CARGO_TARGET_DIR:-target}/release/libimre_runtime.a" \
  --clang clang-18 --python python3.14 --cpu 30 --rounds 8 --passes 12 \
  --sizes 10000 1000000 --output /tmp/struct-points.json
```
