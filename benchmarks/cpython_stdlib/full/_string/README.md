# Format string iterators

Imre implements the two private builtins that CPython's public
`string.Formatter` uses: `formatter_parser` and `formatter_field_name_split`.
Each result owns its input and advances on `next()`. A bad initial integer
raises from the split call; a malformed later component raises on the pull that
reaches it. Continuing after that exception resumes at CPython's cursor position.
Index conversion uses Unicode decimal digits and the platform's signed-size bound.

## Upstream evidence

The reference is [CPython 3.14.5](https://github.com/python/cpython/tree/5607950ef232dad16d75c0cf53101d9649d89115).
The complete original [test_str.py](../str/upstream/Lib/test/test_str.py) and
[test_string package](../string/upstream/Lib/test/test_string) remain unchanged.
Their independent reference ledgers contain 138 tests (132 passes, six skips)
and 54 tests (all pass), respectively.

The two original `StringModuleTest` tests make 11 native operation calls.
The original higher-level `ModuleTest` exercises 14 Formatter cases: 13 call
these native operations, and the recursion-limit case returns before either
operation. CPython runs the fixtures, original bodies, assertions, and other
Formatter behavior. The child executable supplies the first key, actual iterator
rows and exact exceptions. The host replays those values on the corresponding
pulls. Corrupting each of the two builtin results fails its original assertion.
See [the per-test operation counts](cursor-operations-linux.json). This gate also compiles
the unmodified public `Formatter.parse` method natively with a fixture driver.

The ordinary and reachable full-suite loaders for `test.test_str` now stop
at line 9 (`import codecs`), one line after the previous `_string` blocker.
Zero full-suite bodies ran during that startup check. The unchanged extracted
builtin test methods cannot yet compile directly because their nested helper
functions have unannotated parameters. Exact string calls and statically known
invalid arguments are supported; inputs with an unsupported dynamic layout
are still diagnosed by the compiler.

## Reproduce

Run from the checkout with the compiled toolchain and a test-enabled pinned
CPython. Repeat for `--level 3`, `--reachable-only` or `--sanitize`:

```sh
python3 benchmarks/cpython_stdlib/full/_string/differential.py \
  --compiler /path/to/imre --runtime /path/to/libimre_runtime.a \
  --python /path/to/cpython-3.14.5 --level 0 --originals
```

Without `--originals`, the native vs reference comparison runs directly and
does not need the CPython test package. The corpus uses 13,969 strings plus
embedded newline and carriage-return cases: escaped and nested braces, bracket
keys, Unicode conversions, integer overflow, repeated exhaustion, and recovery
after parser errors. Its 285,328-line cursor trace, argument order and ownership
are compared byte for byte. The regular text suite also runs the alias, lifetime
and wrong-call controls and rejects adjacent unknown imports or bad nested calls.
