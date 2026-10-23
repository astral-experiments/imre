# Payout callback dispatch at loop entry

On the unchanged record-pipeline application, selecting the gross or net target at loop entry sped up 256 full-selection queries by **1.062× gross** and **1.053× net**. The six selective or single-query cases all excluded more than a 1% slowdown. The timed release-profile compiler pair was built against `ddaccf1`. Fresh builds against `eaf1f9a` produced byte-identical complete application ELFs, five module objects and five split LLVM units in each arm. [Measurements and fingerprints](2026-10-23-payout-callbacks.json.gz)

| Mode | Risk filter | Queries | Baseline ms | Candidate ms | Baseline / candidate [95% CI] |
| --- | --- | ---: | ---: | ---: | ---: |
| gross | full | 1 | 7.603 | 7.622 | 1.0038 [0.9977, 1.0086] |
| gross | full | 256 | 13.224 | 12.469 | 1.0625 [1.0564, 1.0643] |
| gross | selective | 1 | 7.623 | 7.597 | 1.0008 [0.9961, 1.0072] |
| gross | selective | 256 | 9.107 | 9.018 | 1.0107 [1.0052, 1.0134] |
| net | full | 1 | 7.611 | 7.599 | 1.0037 [1.0000, 1.0066] |
| net | full | 256 | 13.561 | 12.886 | 1.0535 [1.0495, 1.0584] |
| net | selective | 1 | 7.600 | 7.578 | 1.0042 [1.0013, 1.0088] |
| net | selective | 256 | 9.107 | 9.083 | 1.0062 [1.0012, 1.0111] |

Every process read and parsed 4,096 records, ran the requested queries, printed a checksum and replaced one report. The full risk threshold admits 4,096 records per query and the selective threshold admits 403. A separate instrumented build counted one target test per query in both modes. Across 256 full queries, it made 1,048,576 direct calls to the selected target, with zero calls to the other transform. The baseline made the same number of indirect transform calls. The summary function grew from 1,436 to 2,197 native bytes. The application `.text` grew from 560,647 to 561,415 bytes.

The preregistered acceptance thresholds required the lower speedup bound to exceed 1.04× for dense gross and 1.025× for dense net at 256 queries. All other cells required an upper bound below 1% slower. A preliminary version that kept the target test inside the loop measured 0.994× gross and 1.011× net on the full 256-query workloads.

The uninstrumented binaries used ordinary split-module builds, the same system runtime and Ubuntu Clang/LLD 18.1.3. Both passed the 122 independent payout-oracle cases. We ran 160 paired blocks per cell on AMD EPYC Milan guest CPU 20, with a randomized workload order and both compiler orders in every two successive blocks. Each of the 2,560 timed processes passed stdout, stderr, exit-status and filesystem checks. Elapsed times are medians. Ratios are median within-block speedups with 95% percentile intervals from 10,000 bootstrap resamples of the complete two-block groups. These results cover this CPU, input distribution and application.

The complete schedule was also replayed on CPU 12 of the same host with the freshly built `eaf1f9a` executables. All eight intervals passed the original thresholds again. At 256 queries, dense gross and net sped up 1.068× and 1.056×. Selective gross and net sped up 1.017× and 1.012×. The compressed record contains all 5,120 observations and the confidence intervals from both runs.

## Reproduction

Verify the saved samples using only Python or replay the schedule with two native applications. Set `SRC` to the baseline source checkout. Set `BASE_CLI` and `CANDIDATE_CLI` to the release compiler binaries, each beside its verified Linux toolchain bundle, and `RUNTIME` to their shared runtime archive. The programs compile the same source path. `--check-only` runs both 122-case validations and all 16 scenario warmups without collecting timings. [Replay script](../payout_callback_benchmark.py)

```sh
record=benchmarks/results/2026-10-23-payout-callbacks.json.gz
python3 -B benchmarks/payout_callback_benchmark.py --record "$record"
work=$(mktemp -d)
entry="$SRC/examples/record_pipeline/__main__.py"
RELAR_CACHE_DIR="$work/base-cache" "$BASE_CLI" build "$entry" --module-root "$SRC/examples" --runtime "$RUNTIME" -o "$work/baseline"
RELAR_CACHE_DIR="$work/candidate-cache" "$CANDIDATE_CLI" build "$entry" --module-root "$SRC/examples" --runtime "$RUNTIME" -o "$work/candidate"
python3 -B benchmarks/payout_callback_benchmark.py --record "$record" --repo "$SRC" \
    --baseline "$work/baseline" --candidate "$work/candidate" --cpu "$CPU" \
    --work "$work/inputs" --output "$work/replay.json"
```
