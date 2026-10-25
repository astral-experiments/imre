# Collections execution evidence

The [baseline table](baseline-c76a979.tsv) has one row per original ID on
Imre `c76a979397dfe052f46137fcbb556af9d242d066`. Each row was reconciled at
`-O0` and `-O3` against CPython `5607950ef232dad16d75c0cf53101d9649d89115`.

| Exclusive result on that Imre revision | IDs |
| --- | ---: |
| Complete native body, operations and native assertions | 3 |
| Complete native body with CPython fixture hooks only | 0 |
| Complete native doctest body with host output checking | 2 |
| Host-driven native operation | 0 |
| Mixed host/native product operations | 0 |
| Reference only: upstream body is `pass` | 3 |
| First compiler diagnostic | 629 |
| Original method decorator requires fixture support | 19 |
| Executable failed | 0 |
| Upstream reference skip | 3 |
| **Total** | **659** |

`TestCounter.test_total` also passed in the shared runner under CPython's
original fixture hooks. It already has a fully native standalone result and
is counted once. Three upstream `TestSequence` bodies run successfully but do
nothing. Their success provides no evidence for deque indexing or slicing.

`routines.py` follows the loaded case's method to its defining class and
verified source file, including inherited methods. It uses the shared
`suite_native.native_method` and `run_in_fixtures`. The child has native
unittest assertions and selected source imports and factory bindings; the
host retains the original module, class and instance setup/teardown.
Unsupported fixture dependencies and first compiler diagnostics are recorded,
not treated as skips. These first errors do not imply that fixing the error
will make the whole method pass. A newly executable routine still requires an
audit of its tested operation.

Run `routines.py` under the test-enabled pinned Python with `--imre`,
`--runtime`, `--clang`, `--compiler-revision`, `--opt-level 0|3`, and
`--result FILE`. Use the same tools and levels with
[`native_counter.py`](../../native_counter.py) and the
[`doctests.py` gate from #528](https://github.com/astral-sh/imre/blob/25c9a440c90b125db22be3e534f3a853e9258a94/benchmarks/cpython_stdlib/full/collections/doctests.py).
Pass the two outputs of each to `evidence.py --routines O0 O3 --counter O0 O3
--doctests O0 O3 --result baseline.tsv`. It rejects mismatched optimization
results or missing reference IDs.

## Installer namedtuple gate

On installer commit `cd938c5f413a65f57e02e8202c4e1f5f11373f23` in
[#497](https://github.com/astral-sh/imre/pull/497), both
`tests/fixtures/containers/namedtuple.py` (22 output lines) and
`tests/fixtures/module_values/namedtuple/main.py` (3) matched the reference
at both levels. These exercise module-level declarations, fields,
representation and tuple equality.

The same [23 complete upstream methods](namedtuple-cd938c5.tsv) were probed
with `routines.py --class TestNamedTuple`. Twelve first stop at a local
factory call, six require their original decorators, and five have other
first diagnostics. None earns native ID credit on this installer revision.
For example, `test_instance` starts with a local factory, then checks keyword
construction, arity, `_make`, `_replace` and `_asdict`. The module-level
gate cannot establish those behaviors.
