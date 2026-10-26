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

Both original `WarningsDisplayTests.test_formatwarning` and `test_showwarning`
also run in each C and Python fixture. CPython creates the input file, sets up
the warnings context and runs the unchanged assertions. Imre formats the
actual filename and line from that fixture. For `showwarning`, Imre writes to
a real native StringIO; the adapter transfers its contents into the fresh host
StringIO that the original method reads. Returning the wrong text or writing
the wrong contents makes the corresponding original assertion fail.

The full ledger records **six host-driven native operations, 175 reference-only
passes and six upstream skips** at both O0 and O3.

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

Default `formatwarning` and `showwarning` share the public linecache and its
mutable source lists. Omitted and explicit lines preserve CPython's whitespace
and newline behavior. `showwarning` can write to stderr, stdout, text files
and StringIO. OS write errors are ignored; a closed-stream ValueError or a
formatting error from the message's `__str__` propagates.
The display slice accepts builtin category objects, string filenames and source
lines, and supported scalar or exception messages. User-defined warning
classes, arbitrary streams, and replacements for the format hook are outside
this slice. `display_operations.py` compares process status, stdout, stderr,
and the actual file bytes independently of the upstream assertions.

Run the complete evidence comparison with a test-enabled CPython 3.14.5 on Linux:

```sh
python benchmarks/cpython_stdlib/full/warnings/run.py \
  --cpython-source /path/to/cpython \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a \
  --clang /path/to/clang --result /tmp/test-warnings.json
```

## Complete suite startup

`startup-linux.json` records the original filter slice: it runs 22 intact entry modules through both plain import
and the unittest runner, each with and without reachable-only compilation.
Allowing the `warnings` module in regular mode (the prerequisite subprocess
warning support) advances 18 of the 88 checks: eight entries using
`test.support`, plus the direct import in `test_fnmatch`. The actual filter
implementation adds no further change to the first diagnostic in those 88
checks. At that revision, `test.test_warnings` stopped at line 2, `import linecache`.

[The display slice checks](display-startup-linux.json) compare the full pinned
warnings and linecache entries with their runners in both compiler modes.
They had the same first diagnostics before and after adding display:
`importlib` in ordinary warnings startup, `typing.overload` in reachable
warnings startup, and `threading` in linecache startup. Zero whole-suite
methods execute in these eight commands.
