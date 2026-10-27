# CPython 3.14.5 itertools

The unchanged [test_itertools.py](upstream/Lib/test/test_itertools.py) and
[license](upstream/LICENSE) match the Git blobs in [UPSTREAM.json](UPSTREAM.json).
This is one test module, with no package submodules. Its `load_tests` hook also
loads the `itertools.batched` doctest from the reference module's docstring.

The [Linux reference run](reference-linux.json) lists all **137 IDs** (136
unittest methods and one doctest). At default resources 132 pass and five skip.
Running the same suite with `-u cpu` also passes
`RegressionTests.test_long_chain_of_empty_iterables`: **133 pass, four skip**.
The remaining four are the `@support.bigaddrspacetest` methods for product,
combinations, combinations with replacement, and permutations. The decorator
reports insufficient address space on this 64-bit platform. All remain in the
denominator. No Imre result is inferred from the reference run.

From the repository root, with the test-enabled pinned interpreter:

```sh
"$CPYTHON" -B benchmarks/cpython_stdlib/suite_inventory.py \
    benchmarks/cpython_stdlib/full/itertools/UPSTREAM.json --result /tmp/itertools-reference.json
"$CPYTHON" -B -m test --single-process --randseed 1 -u cpu --junit-xml /tmp/itertools-cpu.xml test_itertools
```

## Native execution

[native.py](native.py) copies complete undecorated methods from the verified
source and compiles each with an assertion adapter. The adapter checks equality
and returns an assertion count from the executable. Both the method and every
operation it invokes run in Imre; CPython does not supply iterator results.

The [O0](native-O0.json) and [O3](native-O3.json) standalone controls pass eleven
complete bodies: four repeat methods, four examples of chain/count/cycle/islice,
the twenty-million-item backward tee deletion regression, and two builtin
filter/zip examples. Their native fixture records assertion counts.
The [per-occurrence reports](COVERAGE.md) separately run the original unittest
fixtures and identify where each body, assertion, product operation and fixture ran.
The larger repeat test also needs complex numbers and callable exception
assertions; the repeat GC case creates a cycle between a list and its iterator.
Neither is implied by the four passing repeat methods.

```sh
for opt in 0 3; do
    python3 benchmarks/cpython_stdlib/full/itertools/native.py --imre "$IMRE" --runtime "$RUNTIME" \
        --clang "$CLANG" --opt-level "$opt" --result "/tmp/itertools-O$opt.json"
done
```

[repeat.py](repeat.py) separately compares interleaved pulls from the same
cursor, a live mutable element, repeated exhaustion, eager evaluation of the
default in `next()`, and constructor overflow with CPython. It adds no suite ID.


[sources.py](sources.py) additionally compares partial consumption through two
aliases, a stop before the start, live list changes during cycle's first pass,
mutation during islice argument evaluation, release after a dictionary size
error, and the timing of float step conversion. These differential checks are
outside the denominator. The various-iterator-argument methods still require
Python-defined iterator protocols, including sources that raise mid-pull.

Runtime-loaded Python can import `itertools.count`. CPython's `weakref.finalize`
constructs one counter in its class body and consumes successive indices from it.
The boxed fixture checks shared cursor state, arbitrary-size integer steps, mixed
integer and float steps, and a failed float conversion on two successive pulls.
The original `TestBasicOps.test_count` fixture also imports `islice` and stops at
that import before its method runs.
[CPython source](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/weakref.py#L461)
and [boxed fixture](../../../../tests/fixtures/boxed_itertools_count.py).

Werkzeug imports `itertools.repeat` when loading its data structures. Its
`ImmutableDictMixin.fromkeys` supplies a finite key iterator and an unbounded
`repeat(value)` to `zip`. The boxed fixture runs that dictionary initialization
with a mutable value and checks that every key refers to the original object.
It also checks finite exhaustion, size hints and exceptions from `__index__`.
[Boxed repeat fixture](../../../../tests/fixtures/boxed_itertools_repeat.py)

[tee.py](tee.py) compares independent positions, forks of a partly consumed
fork, a discarded lagging fork, source mutation, and an empty result that must
not obtain an iterator. The larger `test_tee` body also requires weak references,
dynamic types and arbitrary Python source iterators; it is not counted.

## Whole-file startup: starred arguments in an immutable tuple

The pinned [threading helper](../shared/upstream/Lib/test/support/threading_helper.py#L277)
constructs `(func, *args)` before it calls `threading.Thread`. On Imre
`c5d2fe7`, both the import and the runner stopped on that display, with or
without `--reachable-only` ([baseline](tuple-spread-startup-before-linux.json)).
With runtime tuple spreads in function bodies, all four advance to the
unsupported `import _thread` at line 1. The compiler retains its existing
restriction on starred tuple displays in module initializers, class fields,
and parameter defaults. These are compilation checks: **zero upstream methods executed**.

[tuple_spread_audit.py](tuple_spread_audit.py) verifies the helper against its
pinned Git blob and repeats all four checks with the vendored and original
sources. It separately compares the [native exercise](tuple_spreads.py) with
CPython at O0 and O3, in ordinary and reachable-only builds. The [Linux
result](tuple-spread-controls-linux.json) records 38 matching lines per build, including mutation
between spreads, partial iteration on failure, invalid operands, callable
retention, a partially consumed iterator, and a 1,025-element input. It also records how the two checking
modes treat an unused function. These controls add no occurrence to the 137
primary or one companion test.

[whole_files.py](whole_files.py) also builds both original test files directly,
with their own `unittest.main()` entry points. It checks each staged file against
its pinned Git blob and records compiler and runtime hashes, output and exit
status at O0/O3 in ordinary and reachable-only builds. The [Linux
receipt](whole-files-linux.json), from compiler tree
`0b1a0563e2970b4b47ab9e691a5ef6cc2bc32626`, records `_thread` as the primary blocker and
`threading` as the companion blocker in all four modes. These eight builds
executed zero upstream bodies.

```sh
python3 benchmarks/cpython_stdlib/full/itertools/whole_files.py \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a \
  --clang /usr/bin/clang-18 --cpython-git /path/to/cpython \
  --result /tmp/itertools-whole-files.json
```
