# CPython collections suites

The Linux reference has **659 distinct test IDs** at CPython 3.14.5
(`5607950ef232dad16d75c0cf53101d9649d89115`). These are the suites dedicated
to collections and its containers under `Lib/test`, including the nested
free-threading test. The loader includes inherited test methods and both
`load_tests` hooks. The 15 docstrings in `Lib/collections/__init__.py` are
upstream test inputs.

The [versioned native evidence](evidence-linux.json) covers all eight suites
at both optimization levels. See [execution and reproduction](routines.md)
and the [dependency inventory](startup.md) for array registration, argparse's
Mapping class base, and the copy protocols exercised by 16 original cases.
The [branch verification runs](runs/README.md) retain full historical reports
for individual compiler trees. Each report has its own O0 and O3 denominator.

On [repaired main `e328ca3`](runs/e328ca39a14d/README.md), both levels recorded **8 fully native / 659**, eight native bodies using CPython fixtures, 14 CPython-driven operations, four cases without a collections operation, three reference skips and 622 blockers. The eighth is `TestBasic.test_init`; this is the only [classification change](runs/e328ca39a14d/role-changes.json) from the seven-method report below. The supplementary `dict.fromkeys` case remains outside this denominator.

On [canonical main `03c8d84`](runs/03c8d84fa4c1/README.md), both levels recorded
**7 fully native / 659**, eight native bodies with host fixtures, 14 host-driven
native operations, four cases without a collections operation, three reference
skips and 623 blockers. The separate [dict.fromkeys dependency](fromkeys/)
does not add an occurrence to this total.

Run [measure.py](measure.py) with a pinned CPython and explicit Imre artifacts
to reproduce the ledger. The [run instructions](runs/03c8d84fa4c1/README.md) show the arguments.

The [docs stack rerun](runs/3da09720e93c/verified-defaults.json) matched all seven original controls and 28 native executions. Its [source and artifact hashes](runs/3da09720e93c/source.json) identify the compiled source and the separate [table check](runs/3da09720e93c/independent-82d6826-table.json).

The [published-docs verification](runs/c2f397dbe55d/verified-defaults.json) repeats the seven CPython controls and 28 native runs on main at c7b1dca. The [receipt](runs/c2f397dbe55d/source.json) identifies the binaries and generated support table.

| Suite | IDs |
| --- | ---: |
| test_collections (containers, ABCs and doctests) | 116 |
| test_defaultdict | 13 |
| test_deque (including its library reference doctest) | 81 |
| test_ordered_dict (including C, pure Python, subclass and mapping fixtures) | 295 |
| test_userdict | 28 |
| test_userlist | 54 |
| test_userstring | 71 |
| test_free_threading.test_collections | 1 |

Run `python -B reference.py --result reference-linux.json` here with a
test-enabled CPython 3.14.5 source build. The runner verifies the checked-in
and executing files against their Git blobs and compares unittest's discovered
IDs with regrtest's results. It enables the CPU resource for the large
namedtuple case. On the Linux x86-64 GIL build, 656 passed. The three skipped
IDs and upstream reasons are in [the reference report](reference-linux.json).
The free-threaded deque ID is retained even though its parent package raises
`SkipTest` before discovery on this build.

Reference outcomes describe CPython only. The existing
[`native_counter.py`](../../native_counter.py) compiles complete upstream
methods with native assertion adapters. [The Counter method gate](methods.py)
also runs seven of those complete bodies under their original unittest fixtures:
total, membership, equality and all four ordering operations. The per-method
[O0](methods-O0.json) and [O3](methods-O3.json) results retain the method hash
and the host fixture role. The same cases are attributed by the full runner.

`deque_methods.py` uses the shared runner to compile the four complete
`test_big_queue_*` and `test_big_stack_*` methods plus `test_setitem`. The
deque operations and the integer or list assertions run in Imre; the original module, class, and
instance fixture hooks run in CPython. The historical
[O0](runs/pr605-7f3772b78ab4/deque-methods-O0.json) and
[O3](runs/pr605-7f3772b78ab4/deque-methods-O3.json) reports record all five
method hashes. Their directory identifies the original PR head; the reports
do not identify the compiler revision.
Pass `--imre`, `--runtime`, `--clang`, `--opt-level`, and `--result` to
the pinned Python build.

`deque_index_host.py` runs the unchanged randomized `test_getitem` and
`test_delitem` with CPython fixtures, bodies, and assertions. Each deque lives
in an Imre process; operations and their errors are returned over pipes.
With random seed 1, the two tests make 60,288 and 3,504 native calls per
level, respectively. The [O0](deque-index-host-O0.json) and
[O3](deque-index-host-O3.json) reports count their operations. Run the same
command-line options as above on Linux. `measure.py` also includes these two
operations with their CPython roles. All seven IDs belong to the 659 above.
