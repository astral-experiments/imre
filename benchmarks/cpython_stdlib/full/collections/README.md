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
`test_big_queue_*` and `test_big_stack_*` methods. Their saved append/pop
methods and integer assertions run in Imre; the original module, class, and
instance fixture hooks run in CPython. The [O0](deque-methods-O0.json) and
[O3](deque-methods-O3.json) reports record the hash of each unchanged method.
Pass `--imre`, `--runtime`, `--clang`, `--opt-level`, and `--result` to
the pinned Python build. These four IDs are part of the 659 above.
