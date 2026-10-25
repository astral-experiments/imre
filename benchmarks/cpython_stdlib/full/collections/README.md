# CPython collections suites

The Linux reference has **659 distinct test IDs** at CPython 3.14.5
(`5607950ef232dad16d75c0cf53101d9649d89115`). These are the suites dedicated
to collections and its containers under `Lib/test`, including the nested
free-threading test. The loader includes inherited test methods and both
`load_tests` hooks. The 15 docstrings in `Lib/collections/__init__.py` are
upstream test inputs.

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
methods with native assertion adapters. Other IDs require independent Imre
execution evidence before they can count as native.
