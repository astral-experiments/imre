# Collections execution evidence

The [versioned per-occurrence report](evidence-linux.json) covers the same
659 original IDs separately at O0 and O3 on Imre
[`8bed1fe03437`](runs/8bed1fe03437/source.json), using merged deque support
from [#561](https://github.com/astral-sh/imre/pull/561). The
[O0](runs/8bed1fe03437/routines-O0.json) and [O3](runs/8bed1fe03437/routines-O3.json)
routine runs retain the first diagnostic or execution for each ID. Each level has thirteen
methods with native bodies, operations and assertions under the original
CPython fixture hooks: seven Counter methods, four 100,000-item deque
queue and stack methods, and the complete deque `test_clear` and
`test_maxlen_zero`. The last test checks that construction, `extend`, and
`extendleft` consume the source iterator even when no items can be stored.
On the earlier `d041aeb3fd05` tree, independent native assertion controls reported `0 != 3` after clear and
`[] != [0]` after a zero-capacity extension at both optimization levels.
Twelve native Counter doctest bodies have host
output checking, including the arithmetic methods and `most_common`. One additional method,
`test_subtract`, runs its original body and assertions in CPython with 24
native Counter operations. Its direct body compilation blocker is retained
in the same record. The shared classification places those twelve doctests and
that host-driven method in `host-driven-native-operation`; their separate roles
show which code ran where. Four routines have no collections operation, and
three have justified reference skips. The other 626 report their first blocker. No
occurrence on this build is classified as fully native.

The four non-product methods are three original deque methods whose bodies
are `pass`, and an inherited UserString case that calls `str.replace` on
built-in strings. Those bodies were compiled and executed.

To reproduce, run `routines.py`, `doctests.py`, and `counter_host.py` at each level under the pinned
Python with the same `--imre`, `--runtime`, `--clang`, full `--compiler-revision`,
`--opt-level`, and a separate `--result` for each gate and level. The doctest
gate writes all results and exits nonzero if any example collection is blocked.
From this directory:

```sh
python3 native_report.py --routines routines-O0.json routines-O3.json \
  --doctests doctests-O0.json doctests-O3.json \
  --host subtract-host-O0.json subtract-host-O3.json --result evidence-linux.json
python3 ../../suite_evidence.py reference-linux.json evidence-linux.json
```

## Earlier standalone and routine evidence

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

On wheel integration `1a03f4affbb17d2fcb74a04da7c5da7cc1f9fc6e`, both
wheel fixtures again matched CPython at O0 and O3: 22 output lines from
the container fixture and three from the imported namedtuple fixture.
The [new run of all 23 unchanged methods](namedtuple-1a03f4a.tsv) still
has 17 compiler blockers and six original-decorator blockers at both levels.
Twelve of the compiler errors are at a factory inside a method. Only
`test_copy` moved: the `copy` import now resolves, and the body stops at
the loop over `copy.copy` and `copy.deepcopy` (line 581).
The separate 11-prompt `collections.namedtuple` docstring stops at
`Point.__doc__` in its second prompt. Neither the 23 methods nor the
docstring earns native pass credit on this revision.
