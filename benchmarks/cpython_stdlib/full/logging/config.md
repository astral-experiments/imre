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
Formatter.

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

## Filename configuration

`basicConfig(filename=...)` installs a real FileHandler. Its default mode appends.
The native root and handler produce the UTF-8 file, message and newline. An empty
filename selects a stream; a path to a directory raises the operating system’s
error. Explicit `None` counts as a supplied option for conflict checks.
Unknown keywords are evaluated in order and reported after installing the handler
and level, matching CPython’s observable state.

The [filename report](basic-config-files-linux.json) adds two complete routines:

| Upstream method | Body and assertions | Logging operation |
| --- | --- | --- |
| `BasicConfigTest.test_encoding` | CPython | Configure, inspect and close the native file handler; emit UTF-8 through the native root |
| `BasicConfigTest.test_incompatible` | Imre | Construct a stream handler, make five configuration calls, and check the four expected `ValueError` exceptions |

Both retain CPython setup, teardown and cleanup. The native child starts at
the root level observed by the fixture. For `test_encoding`, the report
records each native operation and the compiler diagnostic at the original
`assertIsInstance` call. The direct controls add no occurrences. Binary file
mode remains an ordinary implementation failure.

### File encoding and errors

`FileHandler` opens writable text streams with the selected codec and error
policy. The handler and its live stream expose their encoding and errors;
the file stream retains both after close. The [codec control](../../../../tests/fixtures/files/logging_file_codecs.py)
checks strict failures, append mode without a second UTF-8 signature, delayed
CP1251 output, newline conversion, and which invalid options truncate an existing file.

The [file configuration report](basic-config-file-codecs-linux.json) includes the
unchanged `BasicConfigTest.test_encoding_errors` and `test_encoding_errors_default`.
CPython runs their bodies, file reads, assertions and fixture hooks. Imre configures
the root, selects the error policy, emits the record and closes the file. The
separate `test_encoding_errors_none` remains blocked at its original
`assertIsInstance(handler, logging.FileHandler)`. It also assigns a custom `handleError`.

The [file handle ownership check](file-codec-ownership-linux.json) identifies the
runtime archive and both production and test MIR for `open_text_output`. If the
file state or returned `ObjectOwner` cannot be allocated, the FFI returns null
with `MemoryError` pending. Fault injection at O0 and O3 checked the exception,
the bytes written on retry, and the open file descriptors.
