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
