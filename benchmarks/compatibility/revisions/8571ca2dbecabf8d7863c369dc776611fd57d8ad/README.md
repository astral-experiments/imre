<!-- compatibility-scorecard:start -->
### Pinned compatibility scorecard

Revision `8571ca2dbecabf8d7863c369dc776611fd57d8ad` · CPython **3.14.5** · Linux x86-64. These are original-test receipts at this revision, not cumulative credits.

Frozen 45-family cohort: **2,890 pinned receipts** of **16,225 raw upstream originals**, **1,018 observed stock/O0/O3 pass intersections**, **1,018 verified credited pairs**; **13,335 originals have no validated receipt**.

Supplemental pilots, outside the frozen cohort: **0 receipts** of **3 originals**, **0 observed intersections**, **0 verified credited pairs**.

Scripts: **0 of 84 original commands have receipts**, **0 observed intersections**, **0 verified credited pairs**; **84 not run**.

#### Frozen cohort

| Family | Raw originals | Stock | O0 | O3 | Observed stock/O0/O3 passes | Verified credited pairs | Historical mixed-revision credits |
| --- | ---: | --- | --- | --- | ---: | --- | ---: |
| `argparse` | 1,893 | 1,893 pass | 45 pass, 1,848 not run | 45 pass, 1,848 not run | 45 | 45 verified | 1,887 |
| `base64` | 41 | 41 pass | 35 pass, 1 skip, 5 not run | 35 pass, 1 skip, 5 not run | 35 | 35 verified | 40 |
| `configparser` | 344 | 339 pass, 5 skip | 339 pass, 5 skip | 339 pass, 5 skip | 339 | 339 verified | 339 |
| `csv` | 128 | 124 pass, 4 skip | 122 pass, 6 skip | 122 pass, 6 skip | 122 | 122 verified | 122 |
| `dataclasses` | 276 | 275 pass, 1 skip | 273 pass, 1 error, 2 skip | 273 pass, 1 error, 2 skip | 273 | 273 verified | 273 |
| `fnmatch` | 24 | 24 pass | 24 pass | 24 pass | 24 | 24 verified | 24 |
| `glob` | 24 | not run | not run | not run | unknown (not run) | pending (24 staged, unverified) | 22 |
| `hashlib` | 82 | not run | not run | not run | unknown (not run) | pending (82 staged, unverified) | 78 |
| `json` | 216 | not run | not run | not run | unknown (not run) | pending (216 staged, unverified) | 207 |
| `logging` | 272 | not run | not run | not run | unknown (not run) | pending (2 staged, unverified) | 220 |
| `pathlib` | 1,369 | 6 pass, 1,363 not run | 6 pass, 1,363 not run | 6 pass, 1,363 not run | 6 | 6 verified; 1,363 unknown | 968 |
| `urllib` | 178 | 174 pass, 2 error, 2 skip | 174 pass, 2 error, 2 skip | 174 pass, 2 error, 2 skip | 174 | 174 verified | 173 |

#### Supplemental pilots

| Family | Raw originals | Stock | O0 | O3 | Observed stock/O0/O3 passes | Verified credited pairs | Historical mixed-revision credits |
| --- | ---: | --- | --- | --- | ---: | --- | ---: |
| `errno` | 3 | not run | not run | not run | unknown (not run) | unknown (not run) | — |

The [stdlib report](docs/stdlib.md) lists every frozen family, including those not run.

An observed intersection means the same original passed stock, O0 and O3; frozen credited pairs also require the launcher to pass its imported bad-type control and any required source-withdrawal check. Script passes also require independently anchored frozen preimages and reference observations, plus exact exit, stdout, stderr and effect comparisons. Skips, omissions, unsupported diagnostics, exclusions, timeouts and non-stock successes never earn credited pairs. Historical credits come from the [frozen mixed-revision ledger](../../../../benchmarks/cpython_stdlib/full/frozen-stock-intersection-8818.tsv) and are separate from the pinned result. [Receipts and replay instructions](../../../../benchmarks/compatibility/README.md).
<!-- compatibility-scorecard:end -->
