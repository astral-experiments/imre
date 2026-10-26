# Collections on canonical main `0a46708`

The [source](source.json) records the canonical commit, tree, parents, pinned
CPython commit and the SHA-256 of the compiler and runtime. The [module
report](../../evidence-linux.json) has 659 original occurrences at each level.

| Execution | O0 | O3 |
| --- | ---: | ---: |
| Fully native body, assertions, operation and lifecycle | 7 | 7 |
| Native body and operation, host fixture | 8 | 8 |
| Host body calling native operations | 14 | 14 |
| No collections operation | 4 | 4 |
| Upstream reference skip | 3 | 3 |
| Blocked | 623 | 623 |

The [O0](native-default-O0.json) and [O3](native-default-O3.json) default
fixture runs attempt the 640 ordinary methods. The other 19 occurrences
are 16 doctests and three skips from pinned CPython. Five Counter methods
(`test_ge`, `test_gt`, `test_le`, `test_lt`, `test_total`) and two deque
methods (`test_clear`, `test_maxlen_zero`) ran to completion under native
`unittest.main`. Their receipts include the original method hash, staged
program hash, native output and exit status. No other method passed this runner.

The [O0](routines-O0.json) and [O3](routines-O3.json) complete routine attempts,
doctest reports and native operation adapters provide the remaining roles in
[the fixture report](fixture-roles.json). Only the seven independently verified
native lifecycle results supersede observations in that report on the same
binaries. The [previous module report](../43755cbf6e6b/previous-module-evidence-linux.json)
is preserved. The unchanged [dict.fromkeys method](../../fromkeys/runs/0a46708e7a95/)
has its own denominator.

To reproduce the final classification from the raw observations, run
`python -B benchmarks/cpython_stdlib/full/collections/combine_defaults.py benchmarks/cpython_stdlib/full/collections/runs/0a46708e7a95 --result /tmp/collections-linux.json`
from the repository root. The [native default runner](../530ecc81681a/native_defaults.py)
records the observations with `--all-methods` at each optimization level.
