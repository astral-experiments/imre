# fnmatch (POSIX)

This directory pins CPython 3.14.5's complete `test.test_fnmatch`, its reference implementation and license. The [manifest](UPSTREAM.json) records each original Git blob. The Linux [reference run](reference-linux.json) passes all 24 test methods with no skips.

Imre's matcher uses the same component-pattern parser as `pathlib`. `fnmatch` and `fnmatchcase` treat slashes and leading dots as ordinary characters. Bytes compare one byte at a time; Unicode strings compare code points. The filter operations retain the input objects, preserve iteration order, and stop consuming an iterator at the first mixed str/bytes error. `translate` produces CPython's regex spelling, including atomic star groups and reversed ranges. This implementation targets the POSIX platforms supported by the native runtime.

## What ran

| Check | O0 | O3 |
| --- | ---: | ---: |
| Unchanged CPython methods with all fnmatch calls supplied by Imre | 24/24 | 24/24 |
| Calls from those methods | 2,432 | 2,432 |
| Distinct native calls compared with CPython | 2,429 | 2,429 |
| Entire methods run with their original class, native assertions and unittest runner | 8/24 | 8/24 |

The [operation runner](run_operations.py) traces the unchanged suite, compiles its requests, checks return values and TypeError messages, and replays the native answers through the original fixtures and assertions. The [body runner](run_bodies.py) separately compiles each complete original method with its class, required imports and platform flags, and the original `unittest.main()` entry. Five filter cases and the three `test_mix_bytes_str` methods pass this way. The latter exercise mixed string/bytes arguments in `fnmatch`, `fnmatchcase`, `filter` and `filterfalse` using the original callable `assertRaises`. Swapping the public filters makes each of the five filter cases fail. Giving the first mixed-type call a matching string pattern makes each of the other three fail. The remaining 16 methods are [recorded](evidence-linux.json) as host-driven native operations.

The [unmodified whole-file probe](whole-suite-startup.json) now reaches the callable default `fn=fnmatch` in `check_match` at line 16. Neither compilation mode starts the whole suite. The per-method ledger includes the first diagnostic for each method that still needs the host; these are not upstream skips.

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
