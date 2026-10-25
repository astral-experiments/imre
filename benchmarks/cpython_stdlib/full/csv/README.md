# CPython CSV reader evidence

The reference is CPython 3.14.5's complete `test.test_csv`: **128 occurrences**, each with a distinct test ID. On the Linux release build 124 pass and four debug-reference-count tests skip. [The manifest](UPSTREAM.json) uses the same unmodified vendored `test_csv.py` and license as the focused CSV reader gate.

At O0 and O3 the [native evidence](native-linux.json) records:

| Original occurrences | Body | Assertions | CSV parsing |
| ---: | --- | --- | --- |
| 18 Excel reader methods | Imre | Imre | Imre |
| 1 multiline Excel method | CPython | Imre | Imre |
| 6 `Test_Csv` reader methods | CPython | CPython | Imre |
| 99 other passing reference methods | Not run natively | | |
| 4 debug-only reference skips | Not run natively | | |

The first 18 run byte-for-byte upstream definitions. Their native file fixture writes and reads the data, then checks each resulting row and field. The host selects a method and checks that its process completed. For `test_quoted_nl` the original CPython method supplies its input string and list of expected rows to the same native assertion helper; Imre currently reserves the local name `input`. The final six methods run their original Python assertions against 33 observed native reads per optimization level, covering NUL, delimiters, escaping, EOF, strict errors, and line endings. The driver passes each input element as a file so NUL and CRLF survive transport.

Run from the repository root with the pinned test-enabled interpreter. To use a release install without `Lib/test`, add `--cpython-source /path/to/cpython-3.14.5` to the native runner.

```sh
python3.14 benchmarks/cpython_stdlib/suite_inventory.py benchmarks/cpython_stdlib/full/csv/UPSTREAM.json --result /tmp/csv-reference.json
python3.14 benchmarks/cpython_stdlib/full/csv/run.py --imre target/release/imre --runtime target/release/libimre_runtime.a --clang /usr/bin/clang-18 --result /tmp/csv-native.json
python3.14 benchmarks/cpython_stdlib/suite_evidence.py /tmp/csv-reference.json /tmp/csv-native.json
```
