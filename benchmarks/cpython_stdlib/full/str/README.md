# Unicode string predicates

This directory vendors CPython 3.14.5's complete `Lib/test/test_str.py`.
Its shared `Lib/test/string_tests.py` fixture is already vendored under
[`collections`](../collections/upstream/Lib/test/string_tests.py).
[`UPSTREAM.json`](UPSTREAM.json) pins their Git blobs. The Linux
[`regrtest` reference](reference-linux.json) records all **138 IDs**:
132 passed and six skipped (four platform/memory restrictions, two CPU resource
restrictions).

[`tests/text_line_methods_upstream.py`](../../../../tests/text_line_methods_upstream.py)
runs the original `StrTest.test_isalnum` method and its inherited fixture on
CPython. Its eight ordinary string comparisons and its extra-argument
`TypeError` use results from compiled Imre calls. A deliberately incorrect
native result must fail the upstream assertion. The method's eight direct
astral string literals execute on CPython. Independently, the native public
predicate was compared with CPython on **1,112,064 Unicode scalars**, including
all eight of those literals. Lone surrogates are outside Imre's UTF-8 string
representation. These checks run in both compiler modes at O0 and O3.

Bound calls, unbound `str.isalnum(text)`, and stored descriptors use the
same predicate. The text fixtures check descriptor identity, metadata, exact
argument errors, and evaluation of later arguments before a bad call raises.
An unbound receiver whose runtime type cannot be determined is rejected.

The same gate compiles the verbatim `if not …isalnum(): raise ValueError(…)`
branch from each of the pinned `importlib._bootstrap_external` cache helpers.
For 21 optimization tags it compares the native branches with the outcomes
and complete error messages from the full CPython `cache_from_source` and
`source_from_cache` functions. The surrounding native fixture supplies their
input locals; this does not measure importing the full module.
