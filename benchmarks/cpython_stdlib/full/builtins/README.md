# Builtins

The unchanged CPython 3.14.5 `test_builtin.py` ran 147 tests on Linux: 140 passed and 7 skipped. These outcomes also reproduced on an isolated interpreter containing only the staged tests.

`ascii_case.py` runs the original `BuiltinTest.test_ascii` method, fixture, and assertions on CPython. At O0 and O3 it reads 14 operation results from an Imre executable. CPython supplies the other five: two self-containing containers and three strings containing lone surrogates. An incorrect native result fails the original assertion. This partial observation adds zero complete native methods. The complete native suite still stops before discovery at the class-level starred tuple on line 2841, in both normal and reachable-only mode.

The native fixture in `tests/fixtures/fstrings/ascii_repr.py` covers plain and import-aliased builtin calls, f-string `!a` with an empty format specification, exceptions, identity, and dataclass inheritance. Nested records that require callbacks, dynamically rebound builtins, recursive containers, and lone surrogates are outside this slice.

```sh
python3.14 -I benchmarks/cpython_stdlib/suite_inventory.py benchmarks/cpython_stdlib/full/builtins/UPSTREAM.json --result /tmp/builtins-reference.json
python3.14 -I benchmarks/cpython_stdlib/full/builtins/ascii_case.py --compiler /path/to/imre --runtime /path/to/libimre_runtime.a --sanitize --result /tmp/ascii-result.json
```

The separately verified [interactive result binding](../sys/BUILTINS.md) covers reading, assigning and deleting `builtins._`.
