# Collections on repaired main e328ca3

CPython 3.14.5 discovered 659 occurrences. At O0 and O3, eight complete routines ran with native assertions, fixtures, and operations. Eight other native bodies used CPython fixtures; 14 CPython-driven cases exercised native operations. Four cases contain no collections operation, three are reference skips, and 622 remain blocked.

Compared with the [prior canonical report](../03c8d84fa4c1/), only [deque’s constructor case](role-changes.json) changed classification. Its unchanged method calls `deque('abc', 2, 3)` and `deque(1)` under `assertRaises(TypeError, ...)`. The [four-mode replay](verified-defaults.json) covers all eight methods and retains the independent method and program hashes for the original seven. The [constructor controls](deque-constructor-controls.json) include valid constructors and two deliberately false assertions. Native unittest omits the exception class name from its printed uncaught-error message; both runners report the failed expectation.

Run [measure.py](../../measure.py) with CPython 3.14.5, `--compiler-revision e328ca39a14d8f0243f0b50b106a17d32b1e0a2f`, explicit `--imre`, `--runtime`, and a fresh `--output` directory. To compare the seven older cases, also pass `--compare benchmarks/cpython_stdlib/full/collections/runs/519c9003a144/verified-defaults.json`. All eight suites and both optimization levels are included once. The [separate dict.fromkeys method](../../fromkeys/runs/e328ca39a14d/) remains blocked.

The [sealed doctest rerun](sealed-doctests.json) matches all 65 examples during discovery; executing the actual cases remains blocked. The [integer math.prod fixture](math-product.json) matches CPython in all four modes.

The [published table receipt](verified-support-table.json) identifies the resolved binary and page digest. All [eight controls and 32 native executions](../d72d067c6ebc/verified-defaults.json) passed again after integrating the intervening canonical compiler changes.
