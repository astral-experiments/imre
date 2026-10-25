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

The original entry point at main `76d74f9` still hit the pre-parser complexity
guard. [#576](https://github.com/astral-sh/imre/pull/576) cleared its flat f-string
without changing the upstream test. With the original CPython
[`Lib/_colorize.py`](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/_colorize.py#L115)
(blob `d6673f6692f76151b3b226ebe4d7976456a257e5`), both modes then reached
`ThemeSection(Mapping[str, str])`. The [collections coordination note](https://github.com/astral-sh/imre/pull/572#issuecomment-6392290392)
tracks that real subclass requirement. Neither startup observation is counted
as a passing routine.

The post-merge [startup record](startup-linux.json) reruns both the imports
entry point and the original suite runner on main
`f5807a7d6d6edcc55cd01e061b73d097820c614d`, with and without
`--reachable-only`. The module root contained the pinned `Lib/test` and
`Lib/_colorize.py`; Imre supplied argparse and the other intrinsic modules.
All four checks exited 1 with empty stdout and the same unsupported
`collections.abc.Mapping` export on stderr. No upstream case executed.

On merged numeric main `e3145753e23633a9acbc857e9ac8f363560523db`, the
[four startup controls](startup-merged-e314575-linux.json) still exit 1 before
any test executes. Normal compilation stops at the original
`collections.abc.Mapping` import in `Lib/_colorize.py:4`; reachable-only
compilation reaches `dataclasses.Field` on the next line. The controls use the
same pinned files and preserve stderr for both entry points.

The [four controls on main `06d678c`](startup-color-06d678c-linux.json)
reproduce that split. The [collections owner](https://github.com/astral-sh/imre/pull/642#issuecomment-6396766712)
reports that on the Mapping branch all four reach `dataclasses.Field`.
None of these native startup checks executed a test.

After [Mapping support merged](https://github.com/astral-sh/imre/pull/642#issuecomment-6400456745),
the [startup and theme controls](startup-after-mapping-linux.json) tested
canonical main and the exact tree merged in
[#968](https://github.com/astral-sh/imre/pull/968). On each tree, both entry
points at `-O0` and `-O3`, with and without `--reachable-only`, exited 1
with empty stdout. All eight diagnostics point to `dataclasses.Field` at
the original `Lib/_colorize.py:5`. The [hosted run](https://github.com/astral-sh/imre/actions/runs/40212060769)
also compared the Mapping controls against CPython at both levels. The
reference runner passed all 1,893 tests; no native whole-file test started.

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
