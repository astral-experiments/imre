# NullHandler

`logging.NullHandler` inherits the native base Handler's level, formatter, name
and lifetime. Its lock is `None`. Both `emit` and `handle` return without
looking at the record; a subclass overriding `emit` alone does not receive
records from this handler.

[null_handler.py](null_handler.py) compares those operations with the pinned
interpreter. A separate program extracts the entire unmodified
`RecordingHandler.handle` helper from the verified `test_logging.py` blob
and checks that two calls preserve record identity, insertion order, and the
original list. Its typed constructor recreates that helper's list; this
exercise does not run a `LoggerTest` fixture or add a suite occurrence.
