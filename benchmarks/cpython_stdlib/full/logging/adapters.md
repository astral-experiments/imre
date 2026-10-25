# Logger adapters

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
