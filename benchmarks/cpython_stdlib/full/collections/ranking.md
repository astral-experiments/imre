# Counter.most_common

The original [`collections.Counter.most_common` doctest](upstream/Lib/collections/__init__.py#L625)
constructs a Counter from `'abracadabra'` and returns three key/count tuples.
At O0 and O3 its unchanged prompt ran in a native executable. The host
doctest output checker compared the resulting list with CPython's expected
text. This is one existing occurrence of the 659, with a host assertion.

The neighboring 18-example Counter class doctest does not pass. Its first
unsupported operation advances to `c.elements()` in
[`''.join(sorted(c.elements()))'`](upstream/Lib/collections/__init__.py#L562).
Of the 22 original `TestCounter` unittest methods, seven complete bodies
and their assertions ran natively under original host fixtures; the other 15
still failed to compile. The probes used Imre verification commit
`f820e6e82b53ad9fd0688d0db56e363605754ae3` on merged base `43755cb`.

The separate [compiler fixture](../../../../tests/fixtures/containers/counter_most_common.py)
matched the pinned CPython 3.14.5 process on all 768 lines at both levels.
It checks ties on unordered keys, insertion order after deletion and
reinsertion, arbitrary-precision counts and limits, and unchanged result lists
after mutation of the Counter. This fixture adds no upstream IDs.
