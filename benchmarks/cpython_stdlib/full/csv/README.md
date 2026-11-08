# CPython CSV suite on Linux

The frozen CSV ledger records 128 discoveries in the same order as Linux CPython 3.14.5. Its ordinary and explicit runtime-import builds each passed 122 and skipped six at both `-O0` and `-O3`. The ledger identifies its historical compiler and runtime by SHA-256. Public `8dd8ab48` independently reproduced both modes and all 128 outcomes at O0 and O3. The original tests exercise readers and writers, dictionary field order, quoting and newlines, dialect registration, error handling and dialect detection. Imre runs their bodies, assertions, stream fixtures and cleanups. [Pinned source](UPSTREAM.json) · [Reference outcomes](reference-linux.json) · [Intact native evidence](evidence-linux.json)

The pinned CPython release build passes 124 and skips the four `TestLeaks` methods with the reason `requires sys.gettotalrefcount()`. Imre takes the same four skips. Two upstream `cpython_only` decorators also skip `MiscTestCase.test_disallow_instantiation` and `MiscTestCase.test_lazy_import` on Imre. Both report `implementation detail specific to cpython`. The ledger classifies these two as `unexpected-skip` because the reference interpreter runs them.

Run from the repository root with a release Imre compiler and runtime, Linux CPython 3.14.5 and the path to Clang in `CLANG`. The runner builds ordinary executables at O0 and O3. It verifies and stages the pinned test package inside the entry's module root, then calls `unittest.defaultTestLoader.loadTestsFromModule(test_csv)`. It checks the complete ordered discovery and result vectors against the CPython reference.

```sh
suite=benchmarks/cpython_stdlib/full/csv
python3.14 -B "$suite/run.py" --intact --imre target/release/imre --runtime target/release/libimre_runtime.a --clang "$CLANG" --result /tmp/csv-evidence.json
python3.14 -B benchmarks/cpython_stdlib/suite_evidence.py "$suite/reference-linux.json" /tmp/csv-evidence.json
```

The negative control changes one expected field in a temporary copy of `TestDialectExcel.test_single`. It requires both CPython and the ordinary `-O0` binary to run all 128 tests, fail that original assertion and exit with status 1.

```sh
python3.14 -B "$suite/run.py" --intact --negative-control --imre target/release/imre --runtime target/release/libimre_runtime.a --clang "$CLANG" --result /tmp/csv-negative.json
```

## Earlier reader measurements

The focused runner remains available. It recorded 18 fully native Excel reader methods at each optimization level. For one multiline method CPython supplied the input and expected rows. For six other methods CPython ran the assertions against native reads. All 25 IDs appear in the intact suite. [Reader ledger](native-linux.json)

```sh
python3.14 -B "$suite/run.py" --imre target/release/imre --runtime target/release/libimre_runtime.a --clang "$CLANG" --result /tmp/csv-reader.json
```

For the focused runner, add `--cpython-source /path/to/cpython-3.14.5` when the interpreter was installed without `Lib/test`.
