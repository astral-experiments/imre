# Packed struct lists vs frozen dataclasses

With one million independently constructed two-float points, Imre structs built **3.29× faster** and scanned **2.72× faster** than frozen dataclasses. Repeating a single point and replacing one slot peaked at **3.25 MiB** for structs versus **10.5 MiB** for dataclasses. [Raw samples, tool hashes and run order](2026-10-25-struct-points.json).

## One million separately constructed points

| Implementation | Build (ms) | 12 scans (ms) | Peak RSS (MiB) |
| --- | ---: | ---: | ---: |
| Imre struct | 29.98 | 12.32 | 18.25 |
| Imre dataclass | 98.48 | 33.56 | 56.5 |
| CPython struct | 815.7 | 1245.2 | 129.75 |
| CPython dataclass | 570.8 | 1275.3 | 128.75 |

The builder appends `Point(x, x + 1.0)` per index, cycling through 1,024 coordinate pairs. Each scan calls `p.norm2()` and multiplies the result by that pass’s scale. Imre writes a 16-byte struct row directly, without allocating a separate record. The dataclass baseline allocates each record and appends its pointer. CPython uses slotted objects and a regular list in both cases; the compatibility decorator also checks struct constructor argument types.

## Repeating one immutable point

The list is `[Point(3.0, 4.0)] * 1_000_000`. Imre stores one immutable default and a count. The dataclass list needs one object and a million pointers.

| Implementation | Build (ms) | 12 scans (ms) | Peak RSS (MiB) |
| --- | ---: | ---: | ---: |
| Imre struct | 0.018 | 11.67 | 3.25 |
| Imre dataclass | 13.00 | 11.86 | 10.5 |
| CPython struct | 8.02 | 1417.6 | 22.25 |
| CPython dataclass | 7.12 | 1503.5 | 21.5 |

## Replacing one list entry

Both point types are frozen. The benchmark assigns `rows[n // 2] = Point(6.0, 8.0)`, replacing a **list entry**. The original point and every other slot retain their values.

| Implementation | Build (ms) | Replace one entry (ms) | 12 scans (ms) | Peak RSS (MiB) |
| --- | ---: | ---: | ---: | ---: |
| Imre struct | 0.022 | <0.01 | 11.59 | 3.25 |
| Imre dataclass | 12.09 | <0.01 | 11.77 | 10.5 |
| CPython struct | 6.55 | <0.01 | 1395.3 | 22.25 |
| CPython dataclass | 6.40 | <0.01 | 1392.4 | 21.5 |

On the [previous implementation](https://github.com/astral-sh/imre/blob/93c1f002d2e4009f34f1562342823d3b0aa4eebb/benchmarks/results/2026-10-25-struct-points.md#changing-one-repeated-slot), the first differing assignment took 13.08 ms and peaked at 18.25 MiB on Imre: it expanded all one million rows. Now a single edit adds its bits and row to one 64-slot block. Assigning the default bits back removes it. Later writes can still cross a packing threshold and pay the O(n) conversion once. The [density benchmark](2026-10-25-struct-edit-density.md) measures those threshold-crossing writes directly, for clustered and scattered edits.

## Smaller independently constructed lists

At 10,000 elements:

| Implementation | Build (ms) | 12 scans (ms) | Peak RSS (MiB) |
| --- | ---: | ---: | ---: |
| Imre struct | 0.430 | 0.159 | 3.38 |
| Imre dataclass | 0.990 | 0.133 | 3.75 |
| CPython struct | 8.39 | 12.28 | 15.75 |
| CPython dataclass | 4.36 | 12.70 | 15.0 |

## Method and reproduction

- [Source commit 9a64f6138c3c](https://github.com/astral-sh/imre/tree/9a64f6138c3cc7719ccfd7d2d66ff2ac183ae1a5); [fixture](../fixtures/struct_points.py), [harness](../struct_points.py). Only the import and decorator change between the struct and `@dataclass(frozen=True, slots=True)` sources.
- AMD EPYC-Milan Processor, x86_64 Linux, pinned to CPU 30. Python 3.14.5, Ubuntu clang version 18.1.3 (1ubuntu1). Imre uses `-O3 --whole-program`, without ThinLTO. Both executables use the same release runtime and system allocator.
- Eight fresh measured processes per implementation per workload after one warmup each. A balanced order rotates between rounds. Build, replacement and scan times are taken inside the program and exclude compilation, startup, output and teardown. GNU time reports maximum process RSS. Sub-0.01 ms timings are below useful timer resolution.
- Every sample must return the expected length and an exact binary64 checksum independently calculated with integer arithmetic. The replacement changes the unscaled squared norm by `100 - 25`. Shared-host load and cache state influence timing.

From the repository root, choose an available CPU and run outside ptrace:

```sh
cargo +ohm -Zohm-defaults=no build --release --locked -p imre-compiler -p imre-runtime
python3.14 benchmarks/struct_points.py \
  --compiler "${CARGO_TARGET_DIR:-target}/release/imre" \
  --runtime "${CARGO_TARGET_DIR:-target}/release/libimre_runtime.a" \
  --clang clang-18 --python python3.14 --cpu 30 --rounds 8 --passes 12 \
  --sizes 10000 1000000 --output /tmp/struct-points.json
```
