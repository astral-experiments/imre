The stock interpreter was built from CPython 3.14.5 at `5607950ef232dad16d75c0cf53101d9649d89115`. The original typing suite has 701 method IDs and 11 doctest IDs. All 712 passed with zero skips. [Stock IDs and outcomes](stock-pinned.jsonl)

### Intact native discovery

Each entry uses the byte-identical pinned file, SHA-256 `28d17772f523800f02a5bf1e0cc012779d7df1f616fec7cc34d922dcba7ee1cc`. Ordinary here is `imre -B -m discover`. Boxed compiles the same driver using `--runtime-imports` and executes the resulting native program. Each driver emits `START` before calling an original case and logs the skip reason reported by unittest.

| Imre revision | Ordinary | Boxed | Original callbacks |
| --- | --- | --- | --- |
| Public `06c9fd7d` | O0/O3 stop at line 33, missing reveal_type | O0/O3 same | 0 |
| Public `fe7af98c`, including #4085 | O0/O3 stop at line 43, missing Concatenate | O0/O3 same | 0 |
| Integrated `8c8ec31c` | — | O0 stops in mod_generics_cache.py at line 24 | 0 |
| Integrated `4c4f4a6f` | O0 stops in ann_module.py at line 43 | O0 same | 0 |

[First public baseline](https://github.com/astral-experiments/imre/pull/4077) · [Public reveal_type replay](public-fe7af98c-intact.tgz) · [Earlier intermediate imports](import-barriers.md) · [Later ordinary traceback](ann-module-ordinary-intact.tgz)

### Named native runs

Each row exercises original methods in isolation and retains the stock ordinals. None of these rows adds to the intact count or to the historical 107/712 ledger.

| Stock ordinals | Original methods | Imre revision and modes | Outcomes |
| --- | --- | --- | --- |
| 123–127 | The five ConcatenateTests methods | `71e902fc`, ordinary O0/O3 and boxed O0/O3 | 5 pass in every mode, 0 skip |
| 485 | RevealTypeTests.test_reveal_type | `16d16baf`, ordinary O0/O3 and boxed O0/O3 | 1 pass in every mode, 0 skip |
| 515–524 | The five TypeGuardTests and five TypeIsTests methods | `3ebdd019`, ordinary O0/O3 and boxed O0/O3 | 10 pass in every mode, 0 skip |

[Concatenate IDs, outputs and controls](concatenate-all-71e902fc.tgz) · [Final Concatenate conflict checks](concatenate-overlap-68556273.tgz) · [RevealTypeTests original method](reveal-type-original-485.json) · [Independent reveal_type controls](reveal-type-native-controls.json) · [TypeGuard and TypeIs original IDs and four final-head modes](type-predicates-3ebdd019.tgz)

The direct static compiler and filesystem unittest discovery are separate entry points. A synthetic unittest import-failure case cannot establish an original callback. The eager-alias and metaclass controls import dependency modules and do not count as tests from the typing suite.
