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
