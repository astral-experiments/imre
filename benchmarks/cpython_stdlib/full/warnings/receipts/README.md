# CPython warnings suite receipts

The Linux inventory contains 187 original parents. The [intact run](dfe0f18e/) on public revision `dfe0f18e` contributed 155 paired passes. A later public replay added the three in-process parents that failed in that run. Stock and both native builds completed the selected suite on `7e46a592` with exit code 0.

| Original parent | Fix |
| --- | --- |
| `DeprecatedTests.test_no_retained_references_to_wrapper_instance` | Expose function closure cells so the decorator instance can be collected. |
| `DeprecatedTests.test_only_strings_allowed` | Preserve the Python argument validation in `warnings.deprecated`. |
| `PyWarnTests.test_stacklevel_import` | Skip internal import frames when locating the warning caller. |

On `dfe0f18e`, the 19 original parents that launch interpreter children ran again with Imre as `sys.executable`. Thirteen passed on stock and both native levels. For each passing parent, the process-group records identify its native child drivers and the Clang processes that compiled their Python workloads at O0 or O3. The six other parents failed in both native runs. [Child audit and raw records](dfe0f18e-children/)

The [current ledger](parents.tsv) contains 171 paired parents, six stock skips, four additional native skips and six native child failures. All four additional skips come from CPython-only decorators in the original source. The warnings omission count is zero. [Decorator and callback records](cpython-only-gates.json)

The `receipt` column identifies the run used for each row. The last three `*_lines` columns give one-based registration, start, outcome and stop line numbers. For an in-process selected run, use the `stdout` array in the [process records](7e46a592/raw.json). For a child run, use the corresponding `selected_children.stdout` stream in the child archive. Run `python3 -I -B verify.py` in this directory to reproduce the cumulative count.
