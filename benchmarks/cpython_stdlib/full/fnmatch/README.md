# fnmatch (POSIX)

This directory pins CPython 3.14.5's complete `test.test_fnmatch`, its reference implementation and license. The [manifest](UPSTREAM.json) records each original Git blob. The Linux [reference run](reference-linux.json) passes all 24 test methods with no skips.

Imre's matcher uses the same component-pattern parser as `pathlib`. `fnmatch` and `fnmatchcase` treat slashes and leading dots as ordinary characters. Bytes compare one byte at a time; Unicode strings compare code points. The filter operations retain the input objects, preserve iteration order, and stop consuming an iterator at the first mixed str/bytes error. `translate` produces CPython's regex spelling, including atomic star groups and reversed ranges. This implementation targets the POSIX platforms supported by the native runtime.

## What ran

| Check | O0 | O3 |
| --- | ---: | ---: |
| Unchanged CPython methods with all fnmatch calls supplied by Imre | 24/24 | 24/24 |
| Calls from those methods | 2,432 | 2,432 |
| Distinct native calls compared with CPython | 2,429 | 2,429 |
| Entire methods compiled and run with native assertions | 0/24 | 0/24 |

The [operation runner](run_operations.py) traces the unchanged suite, compiles its requests, checks return values and TypeError messages, and replays the native answers through the original fixtures and assertions. The [body runner](run_bodies.py) also attempts every complete original method. The [occurrence ledger](evidence-linux.json) records the result and compiler diagnostic for each attempt. These are host-driven native operations. They do not count as fully native tests.

An [unmodified suite startup probe](whole-suite-startup.json) stops at `import string` before unittest discovers any cases. Individual method probes also encounter callable defaults, bound methods, global platform flags, unittest context managers and list assertions. None is recorded as an upstream skip.

## Reproduce

Use CPython 3.14.5 and its pinned source tree. Supply absolute paths to the Imre compiler, runtime archive, and Clang. The `--cpython-source` option can be omitted if the interpreter already has the pinned test package.

```sh
suite=benchmarks/cpython_stdlib/full/fnmatch
python3.14 "$suite/run_operations.py" --imre "$IMRE" --runtime "$IMRE_RUNTIME" --clang "$CLANG" \
  --cpython-source "$CPYTHON" --result /tmp/fnmatch-operations.json
python3.14 "$suite/run_bodies.py" --imre "$IMRE" --runtime "$IMRE_RUNTIME" --clang "$CLANG" \
  --cpython-source "$CPYTHON" --operations /tmp/fnmatch-operations.json \
  --compiler-commit "$COMPILER_COMMIT" --result /tmp/fnmatch-evidence.json
```

[The native parity fixture](../../../../tests/fixtures/paths/fnmatch_py314.py) separately checks argument evaluation order, side effects, POSIX path arguments, exact bytes, and checked errors. It runs with the existing path tests.
