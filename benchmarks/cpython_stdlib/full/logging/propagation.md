# Handler search through logger parents

The original `LoggerTest.test_has_handlers_no_propagate` sets
`child_logger.propagate = False` and asserts that `hasHandlers()` is false.
Its CPython setup attaches one handler to root and a different one to a
standalone `Logger("blah")`. That standalone object has no parent and is not
the manager's `"blah"` logger. The test itself gets `"blah.child"` through
the manager.

[native_hierarchy.py](native_hierarchy.py) checks the host fixture before
running the complete unmodified method in a child. The native fixture installs
a root handler through `basicConfig` and verifies its presence before the
upstream body executes. The original module, class, and instance hooks stay
on CPython.

The separate [hierarchy differential](hierarchy_values.py) checks empty root,
root propagation, a parent inserted after a child, stopped and resumed
propagation, config removing the root handler, and both fallback and formatted
stderr. This covers the configured root handler. Explicit per-logger handler
lists and delivery to custom handlers need their own execution evidence.
