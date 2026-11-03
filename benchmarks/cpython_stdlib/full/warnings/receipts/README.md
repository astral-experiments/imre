# CPython warnings suite receipts

The Linux inventory contains 187 original parents. The [intact run](dfe0f18e/) on public revision `dfe0f18e` contributed 155 paired passes. A later public replay added the three in-process parents that failed in that run. Stock and both native builds completed the selected suite on `7e46a592` with exit code 0.

| Original parent | Fix |
| --- | --- |
| `DeprecatedTests.test_no_retained_references_to_wrapper_instance` | Expose function closure cells so the decorator instance can be collected. |
| `DeprecatedTests.test_only_strings_allowed` | Preserve the Python argument validation in `warnings.deprecated`. |
| `PyWarnTests.test_stacklevel_import` | Skip internal import frames when locating the warning caller. |

On `dfe0f18e`, the 19 original parents that launch interpreter children ran again with Imre as `sys.executable`. Thirteen passed on stock and both native levels. For each passing parent, the process-group records identify its native child drivers and the Clang processes that compiled their Python workloads at O0 or O3. The six other parents failed in both native runs. [Child audit and raw records](dfe0f18e-children/)

The default filter configuration and conflicting `PYTHONWARNINGS` and `-W` options were replayed through both warning providers on public revision `58f847de`. All four original parents passed on stock, O0 and O3. The pure-Python default-filter parent had already passed on `dfe0f18e`, so this run contributes three new parents. [Child process and compiler records](58f847de/)

On public revision `061b0196`, both `FinalizationTest` parents passed on stock and in native children at O0 and O3. One child emits a warning from a user-defined finalizer. The other two leave a file open through interpreter shutdown, with and without a prior import of `warnings`. The child argv hashes match the three command strings in the original test source. [Shutdown records](061b0196/)

The remaining child parent, `PyWarningsDisplayTests.test_tracemalloc`, passed on stock and public revision `76aed745` at O0 and O3. It checks the warning and source line when tracing is disabled, then checks the two allocation frames when tracing is enabled. For both invocations, the audit checks the generated script against the original test and matches the child driver PID to Clang. [Resource warning records](76aed745/)

The [current ledger](parents.tsv) contains 177 paired parents, six stock skips and four additional native skips. All four additional skips come from CPython-only decorators in the original source. The warnings omission count is zero. [Decorator and callback records](cpython-only-gates.json)

A separate intact run on public `76aed745` loaded all 187 parents using `unittest.defaultTestLoader.loadTestsFromModule`. Stock passed 181 and skipped six. Both native levels passed the same 177 parents and skipped the same ten. All three runs exited successfully and reported the same 18 successful subtests. The child process and compiler records cover all 19 parents that launch Python, including both invocations of the allocation-traceback test. Run `python3 -I -B 76aed745/intact/verify.py` from this directory to reproduce the comparison using the archived stock stream and native toolchain records. [Intact run receipts](76aed745/intact/)

The `receipt` column identifies the run used for each row. The last three `*_lines` columns give one-based registration, start, outcome and stop line numbers. For an in-process selected run, use the `stdout` array in the [process records](7e46a592/raw.json). For `dfe0f18e-children`, use the corresponding `selected_children.stdout` stream in the child archive. For `58f847de`, use `parents/stock/stock/stdout` or `parents/public-58f847de/{O0,O3}/stdout`. For the shutdown and allocation-traceback runs, use the stock or native `selected_shutdown_children.stdout` or `selected_tracemalloc.stdout` stream in the corresponding archive. Run `python3 -I -B verify.py` in this directory to reproduce the cumulative count.
