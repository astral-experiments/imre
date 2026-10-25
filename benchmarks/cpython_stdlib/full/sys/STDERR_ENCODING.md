# Standard error encoding

Standard error snapshots the codec named by `PYTHONIOENCODING` at native startup. Its error policy is always `backslashreplace`; `-I` ignores the setting. Both `sys.stderr` and `sys.__stderr__` use this encoder, including when the runtime writes an uncaught exception or a `SystemExit` message. UTF-8 with a signature emits one BOM per stream, including on an empty first write.

The [encoding probe](stderr_encoding.py) records stderr metadata, bytes and exit status against CPython 3.14.5. It exercises the native registry's ASCII, Latin-1, UTF-8 and UTF-8 with signature. It also runs two original `test_exit` children and checks the final diagnostic line of an uncaught exception. The [Linux results](stderr-encoding-linux.json) are supplemental. The full `test_exit` still stops at a lone surrogate; `test_ioencoding` also needs CP424 and configured stdout encoding.
