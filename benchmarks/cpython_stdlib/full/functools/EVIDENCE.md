# Native functools evidence

The [primary suite](evidence-linux.json) and [free-threading suite](evidence-free-threading-linux.json) record each CPython 3.14.5 test at O0 and O3. Both levels have the same counts.

| Suite | Occurrences | Fully native | Native body, host fixture | Host-driven native operation | Mixed | Blocked |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Primary | 325 | 0 | 18 | 17 | 7 | 283 |
| Free-threading | 4 | 0 | 0 | 0 | 0 | 4 |

[Compiler revision and binary hashes](source-linux.json) · [Execution roles](../../SUITE_RUNNER.md) · [Archived branch reports](https://github.com/astral-experiments/imre/blob/8234cc525f4991c9cd61cd9a486def8f064964dc/benchmarks/cpython_stdlib/full/functools/EVIDENCE.md)
