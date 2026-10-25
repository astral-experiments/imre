> Project naming is normalized to Imre; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Three telemetry error reductions in one traversal

Compiling the unchanged telemetry application with automatic fusion produced **1.49× and 1.47× speedups** over main `f246f53` for 2,048 reports over the same 4,096 aligned sample pairs. At 96 reports the speedups were 1.07× and 1.07×. One of the two one-report intervals includes 1. The 2,048-report hand/automatic ratios were 0.973 and 0.961. The prespecified ±5% margin held on CPU 12. On CPU 20 the interval reached 0.944.

| Guest CPU | Reports | Base ms | Automatic ms | Hand ms | Base / automatic [95% CI] | Hand / automatic [95% CI] |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 12 | 1 | 9.84 | 9.76 | 9.80 | 1.010 [1.001, 1.027] | 0.988 [0.984, 0.995] |
| 12 | 96 | 12.54 | 11.61 | 11.27 | 1.073 [1.063, 1.103] | 0.978 [0.964, 0.989] |
| 12 | 2,048 | 67.05 | 44.78 | 43.65 | 1.494 [1.491, 1.507] | 0.973 [0.968, 0.976] |
| 20 | 1 | 11.07 | 10.82 | 10.73 | 1.017 [0.978, 1.074] | 0.980 [0.949, 1.003] |
| 20 | 96 | 13.56 | 12.59 | 12.40 | 1.074 [1.063, 1.096] | 0.985 [0.956, 1.039] |
| 20 | 2,048 | 67.84 | 46.14 | 44.52 | 1.473 [1.438, 1.488] | 0.961 [0.944, 0.974] |

**Base** compiles the original source with main. **Automatic** compiles the same source path with the candidate. **Hand** uses the base compiler on a copy with a single source edit. In `summarize`, it initializes all three totals to zero and computes `delta = actual.value - expected.value` once for each pair from strict zip. It adds `delta`, `-delta if delta < 0 else delta`, and `delta * delta` to the respective totals. All arms also run the alert, threshold and latest-time queries. The exact unified diff is in the measurement record. [Raw samples, inputs, patch and fingerprints](2026-10-23-telemetry-loop-fusion.json.gz)

A separately instrumented build counted the work performed inside `summarize` and its distance/square helpers for one report. The same counts doubled with two reports.

| Build | List entries read | Sample values read | Subtractions | Additions | Multiplications |
| --- | ---: | ---: | ---: | ---: | ---: |
| Base | 24,576 | 24,576 | 12,288 | 12,288 | 4,096 |
| Automatic | 8,192 | 8,192 | 4,096 | 12,288 | 4,096 |
| Hand | 8,192 | 8,192 | 4,096 | 12,288 | 4,096 |

All timing used ordinary uninstrumented split-module native builds, the same base runtime, and Ubuntu Clang/LLD 18.1.3. Each application had five cold object misses and a warm executable hit that reproduced the ELF bytes. Each passed 258 upstream telemetry cases, the output-device failure case, and seven direct CPython comparisons covering strict zip mismatches and arbitrary-size integer arithmetic.

Each trial launched a fresh process that parsed both files, aligned the samples, computed the requested reports, rendered and replaced the output file, and exited. Every trial checked exit status, stdout, stderr and all files against the oracle. On each AMD EPYC Milan guest CPU we ran 30 paired blocks with all six build orders equally frequent, following one warmup per build and report count. Times are per-build medians. Ratios are medians of within-block time ratios with 95% percentile intervals from 4,000 bootstrap resamples of complete blocks. Across both CPUs this includes 540 measured processes.

## Reproduction

Use a Linux checkout of `f246f53c4a3a59faff2bda17ae880cd1ad12fed8`, its release CLI and runtime archive, the candidate release CLI, and a matching Clang/LLD pair. Set the variables below to those paths. Run the [benchmark driver](../telemetry_fusion_benchmark.py) from the candidate checkout. Each invocation builds all three applications and materializes the seeded input files. Use another available CPU and `--schedule 20` for the second schedule. Pass `--check-only` for the three builds and nine checked invocations.

```sh
record=benchmarks/results/2026-10-23-telemetry-loop-fusion.json.gz
scratch=$(mktemp -d)
python3 -B benchmarks/telemetry_fusion_benchmark.py --repo "$BASE_SRC" --record "$record" \
    --baseline "$BASE_CLI" --candidate "$CANDIDATE_CLI" --runtime "$BASE_RUNTIME" \
    --lld "$LLD" --work "$scratch/run" --cpu "$CPU" --schedule 12 \
    --output "$scratch/timings.json"
```
