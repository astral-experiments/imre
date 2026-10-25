# Packed struct lists vs frozen dataclasses

With one million separately constructed two-float points, Imre structs built **3.18× faster** and scanned **2.67× faster** than Imre frozen dataclasses. With one point repeated a million times, structs used **3.25 MiB** of peak process memory versus **10.5 MiB** for dataclasses. [Raw samples, tool hashes and run order](2026-10-25-struct-points.json).

## One million separately constructed points

| Implementation | Build (ms) | 12 scans (ms) | Peak RSS (MiB) |
| --- | ---: | ---: | ---: |
| Imre struct | 29.36 | 12.02 | 18.0 |
| Imre dataclass | 93.26 | 32.15 | 56.5 |
| CPython struct | 744.0 | 1221.8 | 129.8 |
| CPython dataclass | 530.4 | 1217.7 | 128.8 |

The loop appends `Point(x, x + 1.0)` once for each index, cycling through 1,024 coordinate pairs. Each scan calls `p.norm2()` and multiplies the result by that pass’s scale. The struct build writes two floats into a 16-byte row. The dataclass baseline allocates a separate record and appends its pointer. CPython uses slotted objects and a regular list in both cases; the struct decorator additionally checks constructor argument types.

The emitted struct build loop has no record allocation. The struct scan calls `imre_struct_read_view` once before the loop to obtain a data pointer and an index mask. Dense lists use the actual index; repeated lists use row zero.

## Repeating one point

The list is `[Point(3.0, 4.0)] * 1_000_000`. Imre stores the immutable point once, plus a logical length. An ordinary dataclass list needs one object and a million pointers. In the [previous measurement](https://github.com/astral-sh/imre/blob/5b374c1084bc8409bd9ac465d257db333889df37/benchmarks/results/2026-10-25-struct-points.md#repeating-one-existing-point), Imre eagerly allocated a million 16-byte rows and peaked at 18.0 MiB.

| Implementation | Build (ms) | 12 scans (ms) | Peak RSS (MiB) |
| --- | ---: | ---: | ---: |
| Imre struct | <0.01 | 11.52 | 3.25 |
| Imre dataclass | 11.05 | 11.63 | 10.5 |
| CPython struct | 7.04 | 1371.8 | 22.3 |
| CPython dataclass | 6.24 | 1371.4 | 21.5 |

## Changing one repeated slot

The program constructs the same repeated list, then replaces its middle slot with `Point(6.0, 8.0)`. Timing the write separately shows the remaining tradeoff. A dataclass list already has a pointer in every slot. Imre must first reserve and fill the dense payload; that one write took 13.08 ms and peak RSS reached 18.25 MiB. Later indexed writes to the dense list update individual rows. Replacing a repeated slot with identical field bits does not expand it.

| Implementation | Build (ms) | First changed write (ms) | 12 scans (ms) | Peak RSS (MiB) |
| --- | ---: | ---: | ---: | ---: |
| Imre struct | <0.01 | 13.08 | 11.82 | 18.25 |
| Imre dataclass | 11.12 | <0.01 | 11.62 | 10.5 |
| CPython struct | 6.37 | <0.01 | 1379.3 | 22.3 |
| CPython dataclass | 6.32 | <0.01 | 1377.1 | 21.5 |

## Smaller independently constructed lists

At 10,000 elements:

| Implementation | Build (ms) | 12 scans (ms) | Peak RSS (MiB) |
| --- | ---: | ---: | ---: |
| Imre struct | 0.410 | 0.112 | 3.25 |
| Imre dataclass | 0.961 | 0.118 | 3.5 |
| CPython struct | 8.03 | 12.03 | 15.8 |
| CPython dataclass | 4.29 | 12.04 | 15.0 |

## Method and reproduction

- Source commit `c6a7a3ab7645`; [fixture](../fixtures/struct_points.py), [harness](../struct_points.py). The only source differences between the struct and dataclass programs are the import and decorator (`@dataclass(frozen=True, slots=True)`).
- AMD EPYC-Milan Processor, x86_64, Linux, pinned to CPU 30. Python 3.14.5; Ubuntu clang version 18.1.3 (1ubuntu1). Imre uses LLVM `-O3 --whole-program`, no ThinLTO, and the same release system-allocator runtime for both executables. This run used rustc 1.99.0-dev (Ohm 1.99.0-7) with its experimental defaults disabled.
- Eight measured processes per implementation per workload, after one unmeasured warmup each. A balanced four-way order rotates between rounds. Times are taken inside the program and exclude compilation, startup, printing and teardown. Sub-0.01 ms phases are shown as `<0.01`; the raw file retains the timer readings. Linux GNU time reports peak RSS for the whole child process.
- Every warmup and sample must print the expected length and an exact binary64 checksum independently calculated with integer arithmetic. For the replacement workload, the expected sum of squared coordinates changes by `100 - 25` before scaling. Shared-host scheduling and cache state influence these results.

From the repository root, choose an allowed CPU and run outside ptrace:

```sh
cargo +ohm -Zohm-defaults=no build --release --locked -p imre-compiler -p imre-runtime
python3.14 benchmarks/struct_points.py \
  --compiler "${CARGO_TARGET_DIR:-target}/release/imre" \
  --runtime "${CARGO_TARGET_DIR:-target}/release/libimre_runtime.a" \
  --clang clang-18 --python python3.14 --cpu 30 --rounds 8 --passes 12 \
  --sizes 10000 1000000 --output /tmp/struct-points.json
```
