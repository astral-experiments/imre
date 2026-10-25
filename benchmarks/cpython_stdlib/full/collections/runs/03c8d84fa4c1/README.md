# Canonical collections measurement

The [compiler and runtime](source.json) were built from canonical main
`03c8d84fa4c199c2429e19efd2c4f5e8781c9f25`. The [659-occurrence ledger](evidence-linux.json)
records seven fully native, eight native bodies with host fixtures, 14 host-driven
native operations, four without a collections operation, three reference skips
and 623 blocked at both O0 and O3. All 659 roles also agree with [canonical
`0a46708`](../0a46708e7a95/evidence-linux.json).

The [23 changes from the prior public baseline](role-changes.json) are listed
by ID, occurrence and fixture. Five host-assisted Counter bodies and two blocked
deque cases now run fully natively. Ten blocked cases have host-driven native
operations; six now have a native body and host fixture. The other 636
classifications are unchanged.

The [seven original methods](verified-defaults.json) passed seven CPython
controls and 28 native executions. The [independent canonical release](../519c9003a144/verified-defaults.json)
has the same method hashes, source blobs, staged IDs, staged-entry hashes
and four native roles. These are repeat observations of the same occurrences.

## Reproduce

Set `CPYTHON` to test-enabled CPython 3.14.5. Set `IMRE`, `RUNTIME`,
`CLANG` and `IMRE_REVISION` to the compiler, runtime archive, linker driver
and exact commit used to build the artifacts. The output directory must be new.

```sh
suite=benchmarks/cpython_stdlib/full/collections
"$CPYTHON" -B "$suite/measure.py" \
    --imre "$IMRE" --runtime "$RUNTIME" --clang "$CLANG" \
    --compiler-revision "$IMRE_REVISION" --output collections-results \
    --compare "$suite/runs/519c9003a144/verified-defaults.json"
```

The runner verifies the upstream blobs, attempts all 640 ordinary methods
at both levels, accounts for the 16 doctests and three reference skips, and
validates the complete ledger. The [separate dict.fromkeys report](../../fromkeys/runs/03c8d84fa4c1/evidence-fromkeys-linux.json)
stays outside 659. For just the seven-method comparison, run
`verify_defaults.py` with the same artifact flags and `--result FILE`.
