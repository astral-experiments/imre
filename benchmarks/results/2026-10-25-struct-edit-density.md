# When to pack sparse struct lists

With `rows = [p] * 1_000_000`, changing one slot keeps one default point plus that slot’s two scalar fields. Scans can reuse one row address for an entire untouched run. Ten thousand adjacent changes still form long runs; ten thousand scattered changes interrupt them. One cutoff based on percentage alone would treat those lists the same.

## Measuring the crossover

This calibration kept the default and changed slots through 50% to measure the cost of staying sparse. The packed control was built by appending the **same** point a million times before editing; it starts with a 16,000,000-byte payload. Both execute identical writes, scans and indexed reads. Each pair below is **sparse / already packed**, using medians of eight processes. Peak RSS includes the whole process.

| Pattern | Changed slots | 12 full scans (ms) | 65,536 indexed reads (ms) | Peak RSS (MiB) |
| --- | ---: | ---: | ---: | ---: |
| Either | 1 | 11.53 / 12.30 | 0.55 / 0.79 | 3.25 / 18.25 |
| Clustered | 10,000 | 11.94 / 12.58 | 0.58 / 0.78 | 3.5 / 18.25 |
| Scattered | 10,000 | 29.65 / 12.81 | 1.99 / 0.82 | 3.75 / 18.25 |
| Clustered | 16,800 | 11.90 / 12.19 | 0.59 / 0.80 | 3.5 / 18.25 |
| Scattered | 16,800 | 31.87 / 12.08 | 1.86 / 0.82 | 4.25 / 18.25 |
| Clustered | 250,000 | 14.17 / 12.07 | 1.23 / 0.80 | 7.25 / 18.25 |
| Scattered | 250,000 | 181.73 / 11.77 | 2.71 / 0.78 | 10.54 / 18.25 |

The clustered schedule changes successive slots from the middle. The scattered schedule visits `(i * 65_537 + n // 2) % n`; each index is unique at this size. The separate read probe visits `(i * 100_003 + 7) % n`. Each test starts from `Point(3.0, 4.0)` and assigns the immutable `Point(6.0, 8.0)`. No field is mutated.

## The implemented rule

Only **adding a new exception** considers packing. Pack if either condition holds:

- **More than 25%** of the logical slots differ.
- **More than 1/64** differ **and at least half** of the 64-slot blocks contain an exception (including the proposed write).

Assigning the original bits back removes an exception. Rewriting an existing exception, reading, or copying an unchanged list does not increase the count. Once dense, the list stays dense. If the dense reservation fails, Imre can still perform the sparse write if it fits.

| Pattern | Changed slots before → after promotion | Last write (ms) | 12 scans (ms) | Peak RSS (MiB) |
| --- | ---: | ---: | ---: | ---: |
| Scattered | 16,799 → 16,800 | <0.01 → 13.75 | 32.79 → 12.27 | 4.25 → 19.0 |
| Clustered | 250,000 → 250,001 | <0.01 → 14.90 | 14.54 → 12.47 | 7.25 → 22.25 |

Each side of an arrow is a separate run; the final assignment is timed on its own. The scattered schedule first touches 7,813 of 15,625 blocks on write 16,800. Clustered edits have only touched 3,907 blocks at write 250,001, so the 25% condition decides. Peak RSS can include both the outgoing sparse allocation and the new 16,000,000-byte buffer during conversion.

At the scattered boundary the calibration saves about 1.65 ms per full scan with packed storage. The measured 13.75 ms conversion would pay back in roughly 9 scans at that rate, before counting faster later writes. This is a workload-specific estimate, not a universal density or a runtime read counter. A workload that makes few reads may still prefer the memory savings of sparse storage.

## Reproduction

[Calibration samples](2026-10-25-struct-edit-crossover.json) · [Shipped-policy samples](2026-10-25-struct-edit-density.json) · [Benchmark program](../fixtures/struct_edit_density.py) · [Harness](../struct_edit_density.py) · [Frozen dataclass comparison](2026-10-25-struct-points.md).

The calibration used source [08a02ac4828b](https://github.com/astral-sh/imre/tree/08a02ac4828b75131ef6bd338f2ad4f33de8f2e8); the shipped policy used [9a64f6138c3c](https://github.com/astral-sh/imre/tree/9a64f6138c3cc7719ccfd7d2d66ff2ac183ae1a5). Both ran on the same AMD EPYC-Milan Processor, x86_64 Linux, CPU 30, using LLVM `-O3 --whole-program` and the system allocator. Each pair alternates order across eight measured rounds, after a warmup per layout. Compilation, process startup and output are outside the timing windows. Linux GNU time reports peak child RSS; sub-0.01 ms timings are below useful timer resolution. Each process must return the expected length, a sum over all rows, and a separate checksum for the indexed reads (computed with the modular inverse, independently of the runtime).

From a release build and outside ptrace, use the commands below. Choose a CPU available on your host.

```sh
# Calibration: at 08a02ac4828b
python3.14 benchmarks/struct_edit_density.py --compiler "${CARGO_TARGET_DIR:-target}/release/imre" \
  --runtime "${CARGO_TARGET_DIR:-target}/release/libimre_runtime.a" --cpu 30 --rounds 8 \
  --counts 0 1 1000 10000 15625 16800 31250 62500 125000 250000 500000 \
  --output /tmp/struct-edit-crossover.json

# Shipped policy: at 9a64f6138c3c
python3.14 benchmarks/struct_edit_density.py --compiler "${CARGO_TARGET_DIR:-target}/release/imre" \
  --runtime "${CARGO_TARGET_DIR:-target}/release/libimre_runtime.a" --cpu 30 --rounds 8 \
  --counts 0 1 1000 10000 15625 16799 16800 31250 62500 125000 250000 250001 \
  --output /tmp/struct-edit-density.json
```
