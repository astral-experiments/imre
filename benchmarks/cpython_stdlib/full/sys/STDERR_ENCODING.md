# Standard error encoding

Standard error snapshots the codec named by `PYTHONIOENCODING` at native startup. Its error policy is always `backslashreplace`; `-I` ignores the setting. Both `sys.stderr` and `sys.__stderr__` use this encoder, including when the runtime writes an uncaught exception or a `SystemExit` message. UTF-8 with a signature emits one BOM per stream, including on an empty first write.

The [encoding probe](stderr_encoding.py) compares metadata, output bytes and exit status with CPython 3.14.5. Its [Linux results](stderr-encoding-linux.json) record 23 supplemental observations at each optimization level, including stream representations, BOM handling and two original `test_exit` children. The last Latin-1 child writes `68 e9 0a` and exits with status 1. The complete method still stops at the preceding child, which contains a lone U+DCFF.

In [the complete occurrence reports](evidence-linux.json), the first `test_ioencoding` child fails because this registry lacks CP424. The three ASCII children match CPython when launched separately. The final child requires a Python string containing U+DCBD. Neither these child controls nor the [stdout](stdout-encoding-linux.json) and [stdin](stdin-encoding-linux.json) controls add upstream occurrences.
