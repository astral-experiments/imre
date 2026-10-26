# shlex

The [manifest](UPSTREAM.json) pins the original CPython 3.14.5 module, test file,
and license. All 19 methods pass on reference Linux CPython.

The [native operation runner](run.py) reruns the original fixture, methods and
assertions. For every public `split`, `join` or `quote` call it compares the
typed result or full exception to CPython, then returns the native answer to
the upstream test. At both O0 and O3, five methods exercise 335 native calls
(273 distinct requests). Of those five methods, the runner also compiles and
executes the unchanged bodies and assertions of `testSplitNone` and `testQuote`.
CPython runs their original `ShlexTest.setUp`; these two [ledger entries](native-linux.json)
therefore have native bodies with host fixtures. The other three use native
operations under host tests, and fourteen methods have reference evidence only.

The same command compares [24,082 additional observations](edge_cases.py)
at each optimization level, including all words up to four characters over
an eight-character alphabet, ASCII inputs, malformed quotes, and an iterable
that must stop at its first invalid element. The stateful lexer class remains
to be implemented.

Run with a test-enabled CPython 3.14.5:

```sh
python benchmarks/cpython_stdlib/full/shlex/run.py \
  --imre target/release/imre --runtime target/release/libimre_runtime.a \
  --clang /usr/bin/clang-18 --result /tmp/native-shlex.json
```
