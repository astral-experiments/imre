# LoggerAdapter.manager

[adapter_properties.py](adapter_properties.py) verifies the pinned
`Lib/logging/__init__.py` blob and extracts the complete decorated getter and
setter without rewriting either method. A native holder and two native adapter
layers exercise identity when reading, replacing, and restoring the shared
manager. The [Linux result](adapter-properties-linux.json) records all five
observations at each optimization level.

This contributes **zero** `test_logging` occurrences. The original
`LoggerAdapterTest.test_nested` also logs through two custom `PrefixAdapter`
objects, inspects a captured record, and replaces the manager with an arbitrary
`object()`. Its real fixture and those operations are still required for that
body to pass.

## Public context and delegation

The [public adapter comparison](../../../../tests/fixtures/files/logging_adapter_core.py)
uses the same logger and dictionaries across calls. It checks that the adapter
retains the caller’s context by identity, leaves unrelated keyword arguments
in place, and returns its input kwargs object. Opt-in merging creates a new
mapping only when both contexts exist. Per-event values take precedence;
later updates to either source stay visible on the next call.

This first implementation accepts a native Logger and dictionaries of string
context values. It also delegates the logger name, threshold, enabled check
and handler search. The control adds no upstream occurrence. Event delivery,
other mapping types, nesting and overrides need separate verification against
the original LoggerAdapterTest methods.

The [Linux receipt](adapter-core-linux.json) records all 29 public observations
at O0 and O3 (identical output SHA-256) and the five unchanged manager checks.
The [focused run](https://github.com/astral-sh/imre/actions/runs/40209023718)
and [full replay](https://github.com/astral-sh/imre/actions/runs/40209029781)
used prospective source `11a468899dc7153afc8cf3ed96b9ef05b7e18745`,
tree `8fed170daef2bc0fd9119ce79f4e726b63669d7b`. The [suite report](evidence-linux.json)
retains all 272 occurrences at each level. Against the prior main-based report,
21 blocked diagnostics moved; classifications and execution roles are unchanged.

## Event delivery

The [focused report](adapter-events-linux.json) covers six original methods.
`test_has_handlers` and `test_extra_not_merged_by_default` run their complete
bodies and assertions in Imre. Four other context methods run their CPython
bodies and assertions against a persistent native adapter, logger and record
store. The shared native equality signature still rejects their string operands.
All six retain the CPython fixture; native Handler.emit captures the original
record where CPython's RecordingHandler captures it in handle.

The [direct comparison](../../../../tests/fixtures/files/logging_adapter_events.py)
covers 38 public outputs. Three other outputs check positive and negative
attribute assertions against CPython. Neither control adds a suite occurrence.
The [full runner](evidence.py) uses the [adapter fixture](native_adapter_events.py) to attempt all eight other adapter
methods at both levels with their unchanged prefix/style helpers.

The [full report](evidence-linux.json) retains 272 occurrences per level:
19 native bodies and assertions with mixed fixtures, four native bodies with
host assertions, 44 host bodies driving native operations (35 host and nine
mixed fixtures), 202 blocked and three reference skips. Six classifications
changed. The [focused hosted run](https://github.com/astral-sh/imre/actions/runs/40214613274)
and [full hosted run](https://github.com/astral-sh/imre/actions/runs/40214612871)
use source `3bbb058b1bc0dd680e720e2c801c9d741797e3dc`,
tree `8adae33110ecabf6d8f395aedbabf39bfffbdbef`.
