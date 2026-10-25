# Base handler names and lifetime

The native `logging.Handler` stores its level, formatter, name, and closed
flag. `logging.getHandlerByName` returns the actual live handler. The name
registry owns weak references: an entry disappears when its last handler
owner drops. A rename or close removes the key associated with the old name,
even when a different handler has claimed that key. Closing preserves the
handler's name, and the same object can subsequently register a new name.

[native_handlers.py](native_handlers.py) runs the unchanged
`HandlerTest.test_name` body under its original `BaseTest` fixtures. CPython
performs both equality assertions and the callable `assertRaises`. The
persistent Imre child creates the handler, performs both property writes and
reads, and raises `NotImplementedError` from the base `emit` method. Each
optimization level reports one host-driven occurrence. The separate
[differential](handler_values.py) checks repeated names, reclamation,
formatters, and numeric and named levels in one process.

The base implementation here covers string names and `None`. The cleanup
list, shutdown callbacks, filters, locks, and other name key types require
separate validation. The callable assertion is tracked with the
[unittest owner](https://github.com/astral-sh/imre/pull/508#issuecomment-6394762377).

The [full suite replay](https://github.com/astral-sh/imre/actions/runs/40187530402) verifies all 272 occurrences at both optimization levels. The HandlerTest.test_name body and assertions run on CPython; the [per-occurrence report](evidence-linux.json) lists its native operations.
