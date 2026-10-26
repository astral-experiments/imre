# Collections branch verification

Each row is one compiler/runtime tree tested against the 659 pinned CPython
3.14.5 occurrences on Linux. The counts in the table were the same at O0 and
O3; the linked reports record both runs separately. The trees contain
different features. Use the [module report](../evidence-linux.json) for the
coordinator's coverage rollup.

| Source and verification | Complete native body, host fixture | Host drives native operation | No collections operation | Reference skip | Blocked |
| --- | ---: | ---: | ---: | ---: | ---: |
| [Deque](f5244fce979c/evidence-linux.json) ([run](https://github.com/astral-sh/imre/actions/runs/40176364237), [PR #561](https://github.com/astral-sh/imre/pull/561)) | 11 | 13 | 4 | 3 | 628 |
| [Mapping ABC](35d411f67cd1/evidence-linux.json) ([run](https://github.com/astral-sh/imre/actions/runs/40176537143), [PR #642](https://github.com/astral-sh/imre/pull/642)) | 7 | 13 | 4 | 3 | 632 |
| [Mapping with the stock argparse consumer](c1033e365c27/README.md) ([run](https://github.com/astral-sh/imre/actions/runs/40187086014), [PR #642](https://github.com/astral-sh/imre/pull/642)) | 7 | 13 | 4 | 3 | 632 |
| [Counter unary and plain dict construction](7e9a095996d0/evidence-linux.json) ([run](https://github.com/astral-sh/imre/actions/runs/40176910489), [PR #868](https://github.com/astral-sh/imre/pull/868)) | 8 | 13 | 4 | 3 | 631 |
| [Deque after callable-cache integration](ff93ef08bb84/evidence-linux.json) ([run](https://github.com/astral-sh/imre/actions/runs/40179123294), [PR #561](https://github.com/astral-sh/imre/pull/561)) | 11 | 13 | 4 | 3 | 628 |
| [OrderedDict](69df05ce8627/evidence-linux.json) ([run](https://github.com/astral-sh/imre/actions/runs/40179772139), [PR #881](https://github.com/astral-sh/imre/pull/881)) | 9 | 13 | 4 | 3 | 630 |
| [Indexed deque and seeded sequences](70966f3b0c03/evidence-linux.json) ([run](https://github.com/astral-sh/imre/actions/runs/40191322080), [PR #605](https://github.com/astral-sh/imre/pull/605)) | 12 | 15 | 4 | 3 | 625 |
| [Direct defaultdict missing-key calls](b4949da4c06a/evidence-linux.json) ([run](https://github.com/astral-sh/imre/actions/runs/40192147528), [PR #976](https://github.com/astral-sh/imre/pull/976)) | 11 | 13 | 4 | 3 | 628 |
| [Mapping with the merged native deque](c42a01682f7b/evidence-linux.json) ([run](https://github.com/astral-sh/imre/actions/runs/40192477050), [PR #642](https://github.com/astral-sh/imre/pull/642)) | 11 | 13 | 4 | 3 | 628 |
| [Merged deque and Counter](8bed1fe03437/evidence-linux.json) ([local build](8bed1fe03437/source.json), [PR #754](https://github.com/astral-sh/imre/pull/754)) | 13 | 13 | 4 | 3 | 626 |

There are zero fully native fixture/body/operation occurrences in these runs.
On the initial deque trees, four of the 11 complete native bodies are the original
100,000-element queue and stack routines; the other seven are Counter methods.
On the Mapping tree, 33 native comparison outputs cover the Mapping subclass
and mixins. All 24 original collection ABC methods are blocked; none of those
33 outputs is a suite occurrence.

The local run on merged deque also credits the complete `test_clear` and
`test_maxlen_zero` bodies, including iterator consumption at zero capacity.
Of its 13 host-driven occurrences, 12 are complete native Counter doctests
whose output is checked by CPython. One runs the original `test_subtract`
body and assertions in CPython against a native Counter. These have different
roles in the report; neither group has a fully native fixture.

On the plain-constructor tree the eighth native body is the original
`TestCounter.test_unary`. Both assertions, both unary operations and both
conversions to plain dictionaries ran natively. The original unittest hooks
ran on CPython. The [O0](7e9a095996d0/unary-controls-O0.json) and
[O3](7e9a095996d0/unary-controls-O3.json) controls check equal mappings in
different orders, an absent zero-valued key, different keys, and different values.

On the OrderedDict tree the ninth native body is the complete untouched
`CPythonOrderedDictTests.test_setitem`. It replaces the value of an existing
key, appends a new key, and compares all six items in order. The
[O0](69df05ce8627/ordered-controls-O0.json) and
[O3](69df05ce8627/ordered-controls-O3.json) assertion controls include
three deliberately wrong orders or values and reject all three. The 30
separate fixture observations do not add occurrences. The newer deque run
also exercises the callable-cache and property-setter fixtures on the same
compiler that ran the four original queue and stack tests.

The [deque method records](70966f3b0c03/deque_methods-O0.json) cover the
complete original `test_setitem` and four 100,000-element methods. Their
bodies, assertions, and operations ran in Imre; CPython ran the unittest
hooks. In the [seeded indexing methods](70966f3b0c03/deque_index_host-O0.json),
CPython ran the bodies and assertions. The live Imre deque received 60,288
operations in `test_getitem` and 3,504 in `test_delitem` at each level.
The archive includes the [O3 native bodies](70966f3b0c03/deque_methods-O3.json)
and [O3 operation counts](70966f3b0c03/deque_index_host-O3.json).

The [29 direct defaultdict observations](b4949da4c06a/defaultdict-four-modes.json)
match CPython in four modes, including a direct `__missing__` call for an
existing key and a call with no factory. All 13 original defaultdict methods
remain blocked on that tree; their first diagnostics are recorded at
[O0](b4949da4c06a/defaultdict-methods-O0.json) and
[O3](b4949da4c06a/defaultdict-methods-O3.json).

The [Mapping controls](c42a01682f7b/mapping-four-modes.json) match 33 outputs
in four modes. All 24 ABC methods remain blocked at
[O0](c42a01682f7b/abc-O0.json) and [O3](c42a01682f7b/abc-O3.json), including
`test_Mapping` at its first instance assertion. The unchanged
[`_colorize.ThemeSection` consumer](c42a01682f7b/theme-section.json) stops at
`dataclasses.Field` before a native Mapping operation. The [original suite
entry points](c42a01682f7b/startup.json) execute zero tests.

Each source directory retains the tested Git tree, base, head, and native
binary digests in `source.json`. Its `fixtures-O*.json` files identify the
separate CPython differential probes. To reproduce an archive, fetch the
verifier branch named in the hosted run, download its `collections-<sha>`
artifact, then run `python -B archive_run.py <download-dir>` from this
module's directory. The script checks Git provenance and validates both full
reports against the pinned reference before copying them.

The refreshed [Sequence ABC run](4435f75200d3/README.md) records both denominators and original assertions at each optimization level.
