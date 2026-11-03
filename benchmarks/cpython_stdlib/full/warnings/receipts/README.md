# CPython warnings suite receipts

The Linux inventory contains 187 original parents. The [intact run](dfe0f18e/) on public revision `dfe0f18e` contributed 155 paired passes. A later public replay added the three in-process parents that failed in that run. Stock and both native builds completed the selected suite on `7e46a592` with exit code 0.

| Original parent | Fix |
| --- | --- |
| `DeprecatedTests.test_no_retained_references_to_wrapper_instance` | Expose function closure cells so the decorator instance can be collected. |
| `DeprecatedTests.test_only_strings_allowed` | Preserve the Python argument validation in `warnings.deprecated`. |
| `PyWarnTests.test_stacklevel_import` | Skip internal import frames when locating the warning caller. |

The [current ledger](parents.tsv) contains 158 paired parents, six stock skips, four additional native skips and 19 parents awaiting verified native children. The warnings omission count is zero.

The `receipt` column identifies the run used for each row. The last three `*_lines` columns give one-based registration, start, outcome and stop line numbers. For the selected run, use the `stdout` array for the corresponding mode in [the raw process records](7e46a592/raw.json). Run `python3 -I -B verify.py` in this directory to verify the cumulative count against both sets of callbacks.
