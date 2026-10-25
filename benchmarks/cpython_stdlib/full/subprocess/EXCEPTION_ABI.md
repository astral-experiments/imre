# Subprocess exception handler controls

The process differential suite compiles [exception_handlers.py](../../../../tests/fixtures/processes/exception_handlers.py)
and compares its output and filesystem effects with CPython. It runs real checked child
exits, timeouts, failed launches, ASCII encoding, a failed typed local assignment, a
dictionary lookup and a same-file copy. An unrelated handler is placed before the
expected handler so an ABI collision causes a failure.

[exception_abi_integration.py](exception_abi_integration.py) adds the parser and import
exception producers once those implementations are available. It was tested on a local
merge of CSV `2ee7d98ec503be69df004f6953624ae4e552c147` and ImportError
`c09151746d94814a43f31b799e74e2086d035b28`. At each of O0 and O3, seven baseline
observations, five integration observations, and the seven observations in the CSV
owner’s [cross-handler control](https://github.com/astral-sh/imre/blob/2ee7d98ec503be69df004f6953624ae4e552c147/benchmarks/cpython_stdlib/full/shutil/csv_exception_handlers.py)
produced the same lines and exit status as CPython 3.14.5. The CSV control's input
paths were an existing regular file and a FIFO. ImportError and ModuleNotFoundError
were constructed and raised by the linked runtime.

The measured compiler SHA-256 was `b7f473807aba7d70b18e704f53c7287b7493eba454c855def9408d3af0f7548c`;
the runtime archive was `8fd568531ffccf8a4924b1dc18bde5910d7ed6d0c1589f3fb47a60724021b5ff`.
These are additional integration controls, outside the 353 upstream occurrences.
