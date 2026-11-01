# fnmatch (POSIX)

This directory pins CPython 3.14.5's complete `test.test_fnmatch`, its reference implementation and license. The [manifest](UPSTREAM.json) records each original Git blob. The Linux [reference run](reference-linux.json) passes all 24 test methods with no skips.

Imre's matcher uses the same component-pattern parser as `pathlib`. `fnmatch` and `fnmatchcase` treat slashes and leading dots as ordinary characters. Bytes compare one byte at a time; Unicode strings compare code points. The filter operations retain the input objects, preserve iteration order, and stop consuming an iterator at the first mixed str/bytes error. `translate` produces CPython's regex spelling, including atomic star groups and reversed ranges. This implementation targets the POSIX platforms supported by the native runtime.

## What ran

| Check | O0 | O3 |
| --- | ---: | ---: |
| Unchanged CPython methods with all fnmatch calls supplied by Imre | 24/24 | 24/24 |
| Calls from those methods | 2,432 | 2,432 |
| Distinct native calls compared with CPython | 2,429 | 2,429 |
| Unmodified suite, direct `unittest.main()` entry | 24/24 | 24/24 |
| Unmodified suite, imported module and `defaultTestLoader` | 24/24 | 24/24 |

The [operation runner](run_operations.py) traces the unchanged suite, checks return values and TypeError messages, and replays the native answers through the original CPython fixtures and assertions. The [body runner](run_bodies.py) compiles each original method with its class, required imports, helper and original `unittest.main()` entry. It also runs separate failure controls for the callable helper and the filter cases.

The [whole-suite runner](run_suite.py) stages the unchanged file and checks both entry points against Linux CPython 3.14.5. Each executable discovers and passes the same 24 methods, with no skips. The [per-occurrence ledger](evidence-linux.json) records the imported suite, including the original fixtures, assertions and fnmatch operations. The [startup checks](whole-suite-startup.json) also accept the imported module and its loader with and without `--reachable-only`.

A separate failure control changes one expected `fnmatchcase` result and two input patterns. Both entry points still run all 24 methods, report the ordinary assertion failure and both failing subtests in order, and exit with status 1 at both optimization levels.

## Reproduce

Use CPython 3.14.5 and its pinned source tree. Supply absolute paths to the Imre compiler, runtime archive, and Clang. The `--cpython-source` option can be omitted if the interpreter already has the pinned test package.

```sh
suite=benchmarks/cpython_stdlib/full/fnmatch
python3.14 "$suite/run_suite.py" --imre "$IMRE" --runtime "$IMRE_RUNTIME" --clang "$CLANG" \
  --result /tmp/fnmatch-whole-suite.json
python3.14 "$suite/run_operations.py" --imre "$IMRE" --runtime "$IMRE_RUNTIME" --clang "$CLANG" \
  --cpython-source "$CPYTHON" --result /tmp/fnmatch-operations.json
python3.14 "$suite/run_bodies.py" --imre "$IMRE" --runtime "$IMRE_RUNTIME" --clang "$CLANG" \
  --cpython-source "$CPYTHON" --operations /tmp/fnmatch-operations.json \
  --compiler-commit "$COMPILER_COMMIT" --result /tmp/fnmatch-evidence.json
```

[The native parity fixture](../../../../tests/fixtures/paths/fnmatch_py314.py) separately checks argument evaluation order, side effects, POSIX path arguments, exact bytes, and checked errors. It runs with the existing path tests.
