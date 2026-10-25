# Doctest in the pinned itertools and JSON suites

Both unchanged `load_tests` hooks call one public doctest function,
`doctest.DocTestSuite(module)`. Its return value must contain real test
cases with the original IDs. The examples within a case share a namespace
and compare their interactive output in order.

| Module passed to `DocTestSuite` | Discovered cases | Examples per case | Occurrences in its reference suite |
| --- | --- | ---: | ---: |
| `itertools` | `itertools.batched` | 1 | 1 of 137 |
| `json` | `json` | 32 | 2 of 216 |
| `json.encoder` | `json.encoder.JSONEncoder.encode` | 2 | 2 of 216 |
| `json.decoder` | None | 0 | 0 |

The JSON package loader includes its two nonempty cases twice. That makes
69 reference example executions across the two suites. These cases belong
to their original module denominators.

[audit.py](audit.py) calls both original hooks, records their actual
constructors and executes each discovered doctest on CPython 3.14.5. It
checks the pinned [source blobs and license](UPSTREAM.json), including the
original modules and the native compiler's copy of each docstring.
[reference.json](reference.json) records the exact source, expected output,
flags, expected exception, indentation and line of each example.

[native.py](native.py) compares discovery and the original example data at
O0 and O3. Separate [execution controls](controls/execute.py) test
interactive display, shared names, failed assignments, exceptions, output
comparison, and the distinction between debug and normal execution. Each
audited module is then tried in its own executable: Imre constructs a suite
and calls its `debug()` method. CPython supplies the comparison transcript;
it never evaluates an example for the native executable. A matching empty
suite contributes zero test cases.

The [Linux execution report](execution-linux.json) was reproduced on repaired
main `fce4734` with this doctest implementation. There,
the complete native `json` case blocks on example 7 (`from io import
StringIO`), the encoder case on example 1 (`from json.encoder import
JSONEncoder`), and the batched case on its module export. Direct suite
controls do not exercise the original package or unittest fixtures.
The [whole-suite probe](../check_suite_startup.py) checks those separately.

With the native `ImportError` implementation on repaired main `fce4734`, all eight
[untouched import and runner probes](startup-linux.json) reach
`Lib/test/support/__init__.py:6` and stop on its `annotationlib` import.
Those probes execute zero test cases.

```sh
dir=benchmarks/cpython_stdlib/full/doctest
"$CPYTHON" -B "$dir/audit.py" --cpython-source "$CPYTHON_SOURCE"
"$CPYTHON" -B "$dir/native.py" --imre "$IMRE" --runtime "$RUNTIME" --clang "$CLANG" --result /tmp/doctest.json
python3 benchmarks/cpython_stdlib/full/check_suite_startup.py --compiler "$IMRE" --cpython-git "$CPYTHON_GIT" test.test_itertools test.test_json
```
