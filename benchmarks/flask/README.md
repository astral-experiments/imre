# Flask over TCP

This gate checks whether a standalone Imre executable can serve the unchanged Flask application and Werkzeug wheels. CPython supplies the expected responses. A native pass requires all ten requests to reach the compiled server on one ephemeral loopback port.

| Request | Expected response |
| --- | --- |
| GET /health | 200, JSON true |
| POST /total with values [2⁹⁵, 3, −2] | 201, total 39614081257132168796771975169 |
| GET /no-route | 404 |
| POST /total with an empty JSON object | 500, with the view's KeyError for "values" in Flask's traceback |
| GET /health | 200 again from the same server |
| GET an escaped Unicode greeting with two suffix values | 200, UTF-8 HTML with the first suffix and both fields escaped |
| GET /visits with a fresh cookie jar | 200, count 1 and a signed session cookie |
| GET /visits with the returned cookie | 200, count 2 and a new signed session cookie |
| GET /visits with one signature character changed | 200, count 1 and a new signed session cookie |
| GET /visits with another fresh jar | 200, count 1 |

The checked-in oracle records status, content type and length, allowed methods, cookie presence, body and its SHA-256 from a prior CPython 3.14.5 TCP session. The client opens real HTTP connections.

The [HTTP performance comparison](comparison/README.md) records throughput and latency for CPython and route-equivalent Node, Bun and Rust controls.

## Reproduce

Use Linux x86-64 with CPython 3.14.5. Acquire the seven release wheels once on a networked machine. The lock pins their published URLs and SHA-256 hashes, including MarkupSafe's CPython 3.14 Linux extension.

    python3.14 -m pip download --no-deps --require-hashes --dest /tmp/flask-wheels -r benchmarks/flask/requirements.txt

After copying the wheelhouse to the test machine, either run just the reference:

    python3.14 benchmarks/flask/gate.py --wheelhouse /tmp/flask-wheels --reference-only --output /tmp/flask-reference.json

or run the complete gate:

    cargo build --release --workspace --locked
    python3.14 benchmarks/flask/gate.py --wheelhouse /tmp/flask-wheels --compiler target/release/imre --runtime target/release/libimre_runtime.a --output /tmp/flask-native.json

Both commands verify the wheel and app hashes and unpack into a fresh module root. The reference runs with system and user site packages disabled and verifies the pinned MarkupSafe extension can load. The native gate builds the same entry point in ordinary and reachable-only modes at O0 and O3. Each successful build must serve the ten responses and expose the original view error. A compile failure records its diagnostic and credits no requests. The full command exits nonzero unless every mode passes.

On Linux, add `--prove-native` to the compiler invocation to require runtime evidence. This opt-in mode requires procfs, `strace` and permission to trace child processes. It verifies that the server process runs the generated ELF, holds an incomplete HTTP request open to identify the process which owns the accepted socket, and checks the listener and process tree. The syscall trace follows worker threads and forked workers, rejects an exec of an unverified image or a mapped `libpython`, and must record termination of the traced processes. The output contains the executable and trace hashes and the socket owners. Full traces are saved beside the JSON report. Missing proof fails the gate.

A separate [typed WSGI environment fixture](../../tests/fixtures/files/boxed_memory_streams.py) stores real memory streams alongside strings, tuples and flags in `dict[str, object]`. The application checks a retrieved value with `isinstance(value, BytesIO)` or `isinstance(value, StringIO)` before using its stream methods. `typing.cast` returns its operand and supplies no runtime class check. Ordinary binary file descriptors and unannotated heterogeneous dictionary inference are outside this fixture's scope.
