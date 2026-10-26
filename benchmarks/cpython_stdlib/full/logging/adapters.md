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

The [focused report](adapter-events-linux.json) covers seven of the 14 original
`LoggerAdapterTest` methods. `test_has_handlers` and
`test_extra_not_merged_by_default` run their complete bodies and assertions in
Imre. Four context methods use CPython assertions to compare the strings read
from native records. `test_is_enabled_for` uses CPython's cleanup stack to
restore the native manager threshold. All seven retain the original CPython
fixture. The remaining seven are attempted by the [full runner](evidence.py);
their individual diagnostics are in the [suite report](evidence-linux.json).

The [public fixture](../../../../tests/fixtures/files/logging_adapter_events.py)
compares 38 outputs with CPython, including the order of argument evaluation,
the overridable enablement and processing hooks, the rerouted record's caller,
and collisions before delivery. The checks also distinguish an omitted
`extra` from an explicit `None`. Three concrete attribute checks exercise
positive and negative assertions. Two deliberately broken recording handlers
run the unchanged default-extra test: dropping its record raises `0 != 1`;
inserting `foo` raises `'LogRecord' object has unexpected attribute 'foo'`.
These controls add no suite occurrences.

At O0 and O3 the complete suite records 25 native bodies and assertions
with mixed fixtures and 55 host-assisted native operations
(4 native bodies with CPython assertions and 51 CPython bodies).
There are 189 blockers and three original reference skips out of 272.
No test fixture is fully native.
