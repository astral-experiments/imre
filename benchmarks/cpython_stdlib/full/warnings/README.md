# CPython 3.14.5: warnings

`upstream/` contains the complete unmodified test package, including its data
modules, plus the corresponding Python and C warning implementations and their
license. `UPSTREAM.json` records every Git blob. The reference interpreter
discovered 187 tests: 181 passed and six skipped because the build's default
warning contexts or thread flags did not meet the upstream condition. The
same outcomes were reproduced in a disposable interpreter whose only test
package was the staged fixture tree.

## What executes natively

At both `-O0` and `-O3`, the unchanged `CFilterTests.test_argument_validation`
and `PyFilterTests.test_argument_validation` methods each make ten native calls:
seven to `warnings.filterwarnings` and three to `warnings.simplefilter`.
CPython supplies each variant's original fixture, test body and assertions.
Imre evaluates the product call and produces the exception; the adapter also
compares its complete class and message to that variant's original function.
The ledger records **two host-driven native operations, 179 reference-only
passes and six upstream skips**. No whole test method is counted as fully
native.

`filters.py` exercises working filters by starting 13 real subprocesses and
comparing which finalizers emit a warning. It checks that reset removes the
initial ignore rule, matching starts at the beginning of the message/module,
messages ignore case, categories and line numbers participate, and moving an
existing filter to the front differs from appending a duplicate. A very large
line number is accepted at registration and produces an unraisable
`OverflowError` only if the C warnings implementation visits that rule.
The comparison checks process labels, warning categories and source lines,
and the overflow class/message. PIDs and absolute interpreter paths vary;
CPython also prints its source line and tracemalloc hint.

The compiler accepts fixed supported Python regular expressions, builtin
warning categories, integer line numbers and integer/boolean `append`.
It rejects dynamic regex syntax, lookaround, and non-ASCII case-insensitive
message patterns. This filter slice does not expose the live `warnings.filters`
list, custom hooks, `catch_warnings`, or general `warn` emission. CPython's
`test.support.ignore_deprecations_from` still needs dynamic patterns before
its entire body can compile.

Run the complete evidence comparison with a test-enabled CPython 3.14.5 on Linux:

```sh
python benchmarks/cpython_stdlib/full/warnings/run.py \
  --cpython-source /path/to/cpython \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a \
  --clang /path/to/clang --result /tmp/test-warnings.json
```

## Complete suite startup

`startup-linux.json` runs 22 intact entry modules through both plain import
and the unittest runner, each with and without reachable-only compilation.
Allowing the `warnings` module in regular mode (the prerequisite subprocess
warning support) advances 18 of the 88 checks: eight entries using
`test.support`, plus the direct import in `test_fnmatch`. The actual filter
implementation adds no further change to the first diagnostic in those 88
checks. `test.test_warnings` itself still stops at line 2, `import linecache`.
Every startup command stops at compile time; zero test methods execute.
