<!-- compatibility-scorecard:start -->
### Pinned compatibility scorecard

Revision `95908fb0acadc2a67304a8080e584deaef395746` · CPython **3.14.5** · Linux x86-64. These are original-test receipts at this revision, not cumulative credits.

Frozen 45-family cohort: **24 pinned receipts** of **16,225 raw upstream originals**, **24 observed stock/O0/O3 pass intersections**, **24 verified credited pairs**; **16,201 originals have no validated receipt**.

Supplemental pilots, outside the frozen cohort: **0 receipts** of **3 originals**, **0 observed intersections**, **0 verified credited pairs**.

Scripts: **4 of 84 original commands have receipts**, **4 observed intersections**, **4 verified credited pairs**; **80 not run**. **4 verified / 72 historically reported stock-success commands** among **84 original script IDs** ([public historical authority](https://github.com/astral-experiments/imre/commit/880648b620f57905a495eb51fbb4600755ee0c54)).

#### Frozen cohort

| Family | Raw originals | Stock | O0 | O3 | Observed stock/O0/O3 passes | Verified credited pairs | Historical mixed-revision credits |
| --- | ---: | --- | --- | --- | ---: | --- | ---: |
| `argparse` | 1,893 | not run | not run | not run | unknown (not run) | unknown (not run) | 1,887 |
| `fnmatch` | 24 | 24 pass | 24 pass | 24 pass | 24 | 24 verified | 24 |

#### Supplemental pilots

| Family | Raw originals | Stock | O0 | O3 | Observed stock/O0/O3 passes | Verified credited pairs | Historical mixed-revision credits |
| --- | ---: | --- | --- | --- | ---: | --- | ---: |
| `errno` | 3 | not run | not run | not run | unknown (not run) | unknown (not run) | — |

The [stdlib report](docs/stdlib.md) lists every frozen family, including those not run.

An observed intersection means the same original passed stock, O0 and O3; frozen credited pairs also require the launcher to pass its imported bad-type control and any required source-withdrawal check. Script passes also require independently anchored frozen preimages and reference observations, plus exact exit, stdout, stderr and effect comparisons. Skips, omissions, unsupported diagnostics, exclusions, timeouts and non-stock successes never earn credited pairs. Historical credits come from the [frozen mixed-revision ledger](../../../../benchmarks/cpython_stdlib/full/frozen-stock-intersection-8818.tsv) and are separate from the pinned result. [Receipts and replay instructions](../../../../benchmarks/compatibility/README.md).
<!-- compatibility-scorecard:end -->
