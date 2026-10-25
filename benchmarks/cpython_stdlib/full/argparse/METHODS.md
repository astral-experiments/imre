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

Separately, 152 source-defined methods in the numeric-head run stop on
`Exception.__init__(self, message, stdout, stderr)` in the unchanged
`ArgumentParserError` fixture. Its separate `error_code` field is not in
the three-element `.args` tuple. For example, `TestAddSubparsers.test_abbreviation` receives the
straight-line exception initializer diagnostic at both levels before its
body can run. The [unittest coordination note](https://github.com/astral-sh/imre/pull/587#issuecomment-6393355465)
tracks the explicit base initializer. These observations remain blocked.
