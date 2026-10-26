# Mapping bases and mixins

The original [`TestCollectionABCs.test_Mapping`](upstream/Lib/test/test_collections.py#L1842)
checks dict registration, abstract methods, comparison and reversed iteration.
At [O0](mapping/73f864ba144c/abc-O0.json) and [O3](mapping/73f864ba144c/abc-O3.json)
its first diagnostic is “unknown class method” at line 1844, the call to
`self.assertIsInstance(sample(), Mapping)`.
All 24 unchanged methods in this test class remain compile-blocked. None has a
native pass in the 659-occurrence ledger. To repeat the per-method probe on a
test-enabled CPython 3.14.5, run `routines.py --class TestCollectionABCs` with
the compiler, runtime, compiler revision, optimization level and output path as
in [the collections runner](README.md).

The separate [compiler fixture](../../../../tests/fixtures/classes/mapping_abc.py)
executes subclass subscription, iteration by terminal `yield from`, length,
truth, `get`, membership, dict's virtual registration and abstract construction.
At O0 and O3, with ordinary and reachable-only compilation, the native executable
produced [the same 33 lines, exit status and stderr](mapping/73f864ba144c/mapping-four-modes.json)
as CPython 3.14.5. The host compares the two processes' output. There
is no host implementation of a Mapping operation in the native process. This
fixture adds no upstream test ID.

For the genuine argparse dependency, stage CPython's unmodified `Lib/_colorize.py`:

```sh
python benchmarks/cpython_stdlib/full/check_suite_startup.py \
  --compiler "$IMRE" --cpython-git "$CPYTHON_GIT" --support-file _colorize.py \
  test.test_argparse test.test_collections
```

On [the tested compiler and runtime](mapping/73f864ba144c/source.json),
[all four argparse checks](mapping/73f864ba144c/startup.json) (import/runner, normal/reachable-only) first stop at
`Lib/_colorize.py:5`, the sealed export `dataclasses.Field`. The actual
`ThemeSection` and its dataclass subclasses do not execute. The independent
[theme probe](theme_section.py) verifies the Git blob, loads the stock module, and
uses its real `Argparse` subclass to exercise Mapping access and iteration.
CPython produced nine observations; [all four native modes](mapping/73f864ba144c/theme-section.json)
stopped at the same Field import before construction. Live Mapping views,
Mapping comparison, dynamic class construction and the ordinary-return iterator
over `self.__dataclass_fields__` remain separate requirements. All four
collections checks still stop at `Lib/test/test_collections.py:3: import array`.
The startup command executes zero tests.
