# Source-defined argparse methods

The CPython 3.14.5 loader discovers 1,893 distinct IDs. Of these, 417 resolve
to a source-defined method; the other 1,476 are metaclass-generated (1,236
parser routines and 240 help routines). This measurement probes the 417 methods
at both optimization levels. The generated IDs remain in the report so it can
be reconciled with the same reference inventory. They are measured separately
by the host-driven adapters.

The [shared runner](../../SUITE_RUNNER.md) compiles the complete original method.
The module-specific fixture includes applicable imports and original argparse
helpers. CPython runs the original module, class and instance hooks, including
the setup that sets `COLUMNS=80`. The child runs the compiled method, parser
operations, and any assertions in the body. It must exit zero with empty stderr
and print its completion marker only after the body returns.

| Source-defined outcomes | -O0 | -O3 |
|---|---:|---:|
| Fully native body, operations and fixtures | 0 | 0 |
| Native body and parser operation, CPython fixtures | 4 | 4 |
| Blocked by the upstream `force_not_colorized` decorator | 77 | 77 |
| Other compiler or fixture blockers | 336 | 336 |
| Failed, skipped or excluded | 0 | 0 |

The four methods are `TestParseKnownArgs.test_arguments_list`,
`test_arguments_tuple`, `test_arguments_list_positional` and
`test_arguments_tuple_positional`. Each makes one unconditional
`ArgumentParser.parse_args` call. The expected behavior is to return without
raising; these four original bodies contain no explicit assertion. The report
includes each method's SHA-256 and each blocker. The remaining 1,476 IDs have
the route label `generated` here, not a pass or exclusion.

To reproduce with the test-enabled release interpreter and the recorded Imre
compiler and runtime:

```sh
python3.14 -B benchmarks/cpython_stdlib/full/argparse/native_methods.py \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang clang-18 \
  --imre-revision "$(git rev-parse HEAD)" --result /tmp/argparse-methods.json
```

## Whole-file startup

Check the import and the standard unittest entry point using the vendored tests.

```sh
python3 benchmarks/cpython_stdlib/full/check_suite_startup.py \
  --compiler /path/to/imre test.test_argparse
```

The checker tries each entry point with and without `--reachable-only`. To
include CPython's original `_colorize.py`, pass `--cpython-git /path/to/cpython`
and `--support-file _colorize.py`. These compilation checks execute zero
upstream tests. To exercise mapping operations on CPython's original
dataclass-backed theme, run the [theme probe](../collections/theme_section.py).

### What the color theme needs at runtime

The [CPython trace](color-trace-linux.json) separates import, the 1,893 original
tests, direct Mapping operations, and an explicit annotation query. During
import the decorators on `Argparse`, `Syntax`, `Traceback` and `Unittest`
each load `Field` once from the [base annotation](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/_colorize.py#L124).
The [dataclass decorator](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/dataclasses.py#L1222)
inspects the generated initializer's signature; annotation resolution reaches
`ThemeSection.__annotate__` even though its annotation is deferred.

Import creates 41 `dataclasses.Field` objects across the five dataclasses.
The `Argparse` section holds 13 named fields. `__post_init__` reads their
names and values, and `no_colors` constructs the corresponding empty strings.
The direct probe checks Mapping iteration order, length, lookup, and `get`
on a hit and a miss. After import, the full reference suite made 10,328
`get_theme` calls with no further load of the `Field` name. All these
observations ran on CPython.

To reproduce, start a fresh test-enabled CPython 3.14.5 with its original
`Lib` and `Tools/i18n` available:

```sh
python3.14 -S -B benchmarks/cpython_stdlib/full/argparse/trace_color.py
```

### Source-defined parser errors

Separately, 152 source-defined methods in the numeric-head run stop on
`Exception.__init__(self, message, stdout, stderr)` in the unchanged
`ArgumentParserError` fixture. Its separate `error_code` field is not in
the three-element `.args` tuple. For example, `TestAddSubparsers.test_abbreviation` receives the
straight-line exception initializer diagnostic at both levels before its
body can run. The [unittest coordination note](https://github.com/astral-sh/imre/pull/587#issuecomment-6393355465)
tracks the explicit base initializer. These observations remain blocked.
