# Flask over TCP

The same unchanged Flask application and Werkzeug server run under CPython and Imre. All five requests target one process on an ephemeral loopback port:

| Request | Expected response |
| --- | --- |
| GET /health | 200, JSON true |
| POST /total with values [2⁹⁵, 3, −2] | 201, total 39614081257132168796771975169 |
| GET /no-route | 404 |
| POST /total with an empty JSON object | 500, with the view's KeyError for "values" in Flask's traceback |
| GET /health | 200 again from the same server |

The checked-in oracle records status, content type and length, allowed methods, cookie presence, body and its SHA-256 from a prior CPython 3.14.5 TCP session. The client opens real HTTP connections.

## Reproduce

Use Linux x86-64 with CPython 3.14.5. Acquire the seven release wheels once on a networked machine. The lock pins their published URLs and SHA-256 hashes, including MarkupSafe's CPython 3.14 Linux extension.

    python3.14 -m pip download --no-deps --require-hashes --dest /tmp/flask-wheels -r benchmarks/flask/requirements.txt

After copying the wheelhouse to the test machine, either run just the reference:

    python3.14 benchmarks/flask/gate.py --wheelhouse /tmp/flask-wheels --reference-only --output /tmp/flask-reference.json

or run the complete gate:

    cargo build --release --workspace --locked
    python3.14 benchmarks/flask/gate.py --wheelhouse /tmp/flask-wheels --compiler target/release/imre --runtime target/release/libimre_runtime.a --output /tmp/flask-native.json

Both commands verify the wheel and app hashes and unpack into a fresh module root. The reference runs with system and user site packages disabled and verifies the pinned MarkupSafe extension can load. The native gate builds the same entry point in ordinary and reachable-only modes at O0 and O3. Each successful build must serve all five responses and expose the actual view error. A compile failure records its diagnostic and credits no requests. The full command exits nonzero unless every mode passes. This opt-in compatibility check does not measure throughput.

A separate [typed WSGI environment fixture](../../tests/fixtures/files/boxed_memory_streams.py) stores real memory streams alongside strings, tuples and flags in `dict[str, object]`. The application checks a retrieved value with `isinstance(value, BytesIO)` or `isinstance(value, StringIO)` before using its stream methods. `typing.cast` returns its operand and supplies no runtime class check. Ordinary binary file descriptors and unannotated heterogeneous dictionary inference are outside this fixture's scope.

## Imre views hosted by CPython

The hosted experiment uses Imre's existing boxed LLVM lowering for the unchanged application module, including its decorators and all four view bodies. CPython runs Flask, Werkzeug, Jinja, session signing and Python object operations, including `sum`. The native functions call a small adapter for the operations this application uses. Flask invokes those functions in the same worker. The launcher checks the application hash and rejects CPython execution of the application's source.

On Linux with LLVM and the CPython 3.14.5 development headers, build the extension and run the same TCP gate:

    python3.14 benchmarks/flask/hosted/build.py --compiler target/release/imre --runtime target/release/libimre_runtime.a --out /tmp/flask-views
    python3.14 benchmarks/flask/gate.py --wheelhouse /tmp/flask-wheels --hosted-launcher /tmp/flask-views/launch.py --output /tmp/flask-hosted.json

The first command records the source, compiler, runtime, LLVM, extension and adapter hashes in `/tmp/flask-views/build.json`. The second stages the seven pinned wheels and compares the hosted worker with CPython on the same five responses. Use the ordinary compiler and runtime flags to measure standalone Imre compatibility.
