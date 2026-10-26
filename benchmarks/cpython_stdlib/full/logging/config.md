# Logger configuration

`native_config.py` runs four complete CPython 3.14.5 methods under the shared
unittest fixture runner. The two `ConfigDictTest` methods pass their original
dictionary to the Imre executable as JSON; the two `ConfigFileTest` methods
pass their real filename. Imre opens the file, validates configuration and
raises the exception. CPython runs the body, assertions and fixture hooks.
The runner also checks the precise native exception type and message: the
two original dictionary tests accept any exception.

`config_values.py` separately compares a single process against CPython,
including repeated dictionary and INI configuration, mutable configuration
passed by reference, custom levels, parent inheritance, propagation, and
disabling and re-enabling an existing logger. This comparison adds no
upstream test IDs.

This first implementation accepts logger and root settings and UTF-8 or
Latin-1 INI paths. Constructing configured handlers, filters, formatters and
custom factories remains open. These are module implementation gaps, not
transport or platform exclusions.

With repaired main `00f91c4`, bounded tuple displays in [#588](https://github.com/astral-sh/imre/pull/588)
and these configuration bindings together (integration `8eccc50`), all four untouched entries (import
and suite runner, full and reachable-only) pass the logging imports and stop
at `Lib/test/test_logging.py:26:1: import codecs`. The one upstream use is
`codecs.getwriter('cp1251')` at line 2306, in the Cyrillic StreamHandler test.
This check executes no test fixture.

After #588 merged, the same four checks on configuration integration `9135966`
(main `1ee7c84`) reached the same `import codecs`. The native codec registry
work in [#691](https://github.com/astral-sh/imre/pull/691) tracks the shared
prerequisite; this test also needs a CP1251 stream writer for the real byte stream.

## Root stream configuration

Public `logging.basicConfig` and the module logging functions install a
real StreamHandler in the root logger’s live list. A second call leaves an
attached handler alone. With `force=True`, configuration snapshots the old
list, removes and closes those handlers, then installs the replacements.
It preserves any caller-supplied formatter. All three styles use the native
Formatter. Filename and filemode configuration remain outside this change.

The [focused Linux report](basic-config-streams-linux.json) covers two complete
CPython 3.14.5 methods at O0 and O3. `BasicConfigTest.test_force` runs all
four original assertions with stock native unittest, including the exact
WARNING and INFO output. `test_datefmt` reads the assigned formatter and
compares its optional string in Imre; the adapter records stock unittest’s
integer contract diagnostic. The original setup, teardown and cleanup run in
CPython. Each native child starts with the observed empty root and saved level.

The [control](../../../../tests/fixtures/files/logging_root_stream_config.py)
also checks handler aliases, caller-owned streams, conflicts, and explicit
None arguments. Run the [adapter](native_basic_config.py) with pinned Python:

```sh
python -I -B benchmarks/cpython_stdlib/full/logging/native_basic_config.py \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
  --cpython-source /path/to/cpython --result /tmp/basic-config-streams.json
```
