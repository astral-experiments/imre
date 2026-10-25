# Collections branch verification

Each row is one compiler/runtime tree tested against the 659 pinned CPython
3.14.5 occurrences on Linux. The counts in the table were the same at O0 and
O3; the linked reports record both runs separately. The trees contain
different features. Use the [module report](../evidence-linux.json) for the
coordinator's coverage rollup.

| Source and hosted run | Complete native body, host fixture | Host drives native operation | No collections operation | Reference skip | Blocked |
| --- | ---: | ---: | ---: | ---: | ---: |
| [Deque](f5244fce979c/evidence-linux.json) ([run](https://github.com/astral-sh/imre/actions/runs/40176364237), [PR #561](https://github.com/astral-sh/imre/pull/561)) | 11 | 13 | 4 | 3 | 628 |
| [Mapping ABC](35d411f67cd1/evidence-linux.json) ([run](https://github.com/astral-sh/imre/actions/runs/40176537143), [PR #642](https://github.com/astral-sh/imre/pull/642)) | 7 | 13 | 4 | 3 | 632 |
| [Counter unary and plain dict construction](7e9a095996d0/evidence-linux.json) ([run](https://github.com/astral-sh/imre/actions/runs/40176910489), [PR #868](https://github.com/astral-sh/imre/pull/868)) | 8 | 13 | 4 | 3 | 631 |

There are zero fully native fixture/body/operation occurrences in these runs.
On the deque tree, four of the 11 complete native bodies are the original
100,000-element queue and stack routines; the other seven are Counter methods.
On the Mapping tree, 33 native comparison outputs cover the Mapping subclass
and mixins. All 24 original collection ABC methods are blocked; none of those
33 outputs is a suite occurrence.

On the plain-constructor tree the eighth native body is the original
`TestCounter.test_unary`. Both assertions, both unary operations and both
conversions to plain dictionaries ran natively. The original unittest hooks
ran on CPython. The [O0](7e9a095996d0/unary-controls-O0.json) and
[O3](7e9a095996d0/unary-controls-O3.json) controls check equal mappings in
different orders, an absent zero-valued key, different keys, and different values.

Each source directory retains the tested Git tree, base, head, and native
binary digests in `source.json`. Its `fixtures-O*.json` files identify the
separate CPython differential probes. To reproduce an archive, fetch the
verifier branch named in the hosted run, download its `collections-<sha>`
artifact, then run `python -B archive_run.py <download-dir>` from this
module's directory. The script checks Git provenance and validates both full
reports against the pinned reference before copying them.
