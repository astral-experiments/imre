# FileHandler execution

The two CPython `FileHandlerTest` methods both unlink the original fixture
filename in the test body. Imre runs that operation along with constructing
the handler, creating the record, writing, flushing, closing, and inspecting
the file or stream. The write-mode test uses the upstream `next_rec` and
`next_message` helpers, sets a native formatter, emits again after close,
and checks that the file contains only the first record.

In both methods the CPython runner supplies the original module, class and
instance hooks, the temporary filename, and initial message counter. The
native child runs typed assertions for the optional stream and text readback.
These are two of the 272 original occurrences at each optimization level.

The differential in `file_values.py` checks UTF-8 output, delayed opening,
stream identity, both text and Path filenames, a changed terminator, and
append-mode reopening. It adds no test IDs. Other encodings and custom
path-like objects still need support from the native stream and path APIs.

Formatting, write, and flush failures reach `handleError` with the original
record and active exception. A delayed open failure or a `RecursionError`
propagates to the caller. The default handler honors `logging.raiseExceptions`.
Imre's diagnostic includes the exception and its explicit causes, the record's
filename and line, and the message and arguments. Native frames have no
traceback objects.

`FileHandler` inherits `StreamHandler`. A logger can attach a file handler,
apply its level and filters, and remove that same handler. Closing releases
the current stream and unregisters the handler name. Replacing the stream
does not close the previous one.

`tests/fixtures/files/logging_file_delivery.py` also compares 25 observations
with CPython at both levels: accepted and filtered record identities, UTF-8
bytes, stream replacement, `fileno`, delayed close without creating a file,
and close delegation through a CP1251 writer. It contributes zero test IDs.
