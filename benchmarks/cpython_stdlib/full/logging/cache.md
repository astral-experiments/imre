# Logger enablement caches

The cache population and `setLevel` invalidation come from [#420](https://github.com/astral-sh/imre/pull/420).
The public `_cache` exposes that store in insertion order, and `logging.disable()`
updates the manager limit and invalidates the same dictionaries.

`LoggerTest.test_caching` checks that child loggers inherit levels, retain both
true and false cache entries, and invalidate all registered loggers on `setLevel`
and `logging.disable()`. The original `LoggerTest` and `BaseTest` hooks remain
on CPython. The fixture attempts the complete method with native assertions first.

If the [shared dictionary assertion](https://github.com/astral-sh/imre/pull/508#issuecomment-6396197563)
cannot compile at upstream line 6220, the original method runs its 29 assertions
on CPython. One native process holds both named loggers and the root. It answers
35 calls, including all 12 cache reads, and survives every level and disable
change in the method. The occurrence report records the precise compile blocker
and where the body and assertions actually ran. A different compiler diagnostic
fails the replay.

`cache_values.py` separately compares dictionary identity and saved references,
explicit cache edits, manager suppression at its boundary, registered level
names, invalid levels, and large integers. It adds no suite occurrences.

The full replay at `303e9fb05462f91c53b4a7c7736876024e988cfe` gives the same result at O0 and O3. At upstream line 6220 the shared assertion expects `bool | int` rather than `dict[int, bool]`. The [per-occurrence report](evidence-linux.json) records CPython running the body and assertions, and Imre handling the cache operations.
