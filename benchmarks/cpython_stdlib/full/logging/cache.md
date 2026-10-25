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

The [full suite replay](https://github.com/astral-sh/imre/actions/runs/40185453005) verifies all 272 occurrences at both optimization levels. The LoggerTest.test_caching body and assertions run on CPython; the [per-occurrence report](evidence-linux.json) lists its native operations.
