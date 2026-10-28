# Frozen Flask / Rust HTTP benchmark

This experiment freezes a restricted Flask application into a statically compiled Imre server and exercises it against the existing Rust/Hyper control. Both servers receive the same HTTP requests from the same Go client. Each request opens a new loopback TCP connection, and the client validates response status, content type and exact body.

## Run it

From an Imre checkout, on Linux x86-64, install CPython **3.14.5**, the repository's Rust toolchain, Go, Clang 18, GNU patch, `strace`, `readelf`, and `taskset`. The native admission requires a host that allows tracing a child process. The first run downloads the seven SHA-256-pinned wheels in [the Flask requirements](../requirements.txt) and the dependencies locked in the two Cargo workspaces.

```sh
python3.14 benchmarks/flask/frozen/run.py --smoke
python3.14 benchmarks/flask/frozen/run.py
```

The first command exercises both native optimization levels and runs one short paired trial. The second runs five alternating-order pairs, with 900 warmups and 36,000 measured requests **per server per round**, at concurrency eight. Both commands build Imre and its runtime from the checkout, build the [locked Hyper server](../comparison/rust) and [Go client](../comparison/client.go), reconstruct the fork, and compile the mixed app and a second catalog app. They print the output directory and save the generated Python, LLVM, executable digests, admission results, and per-round samples there. They fail if compilation, the traced ten-request gate, cross-signing, or a body check fails. The benchmark requires at least two available CPUs. Select them explicitly and reuse downloaded wheels if desired.

```sh
python3.14 benchmarks/flask/frozen/run.py --work ./frozen-results \
    --server-cpu 2 --client-cpus 3,4,5 --wheelhouse ./wheels
```

The output directory must be new. Cargo artifacts use `CARGO_TARGET_DIR` or the checkout's `target` directory. The default client uses up to three other CPUs from the process's affinity mask. Use dedicated server and client CPUs for timing.

## What runs

The benchmark applies [modifications to existing package sources](upstream.patch) to the verified wheels, adds the [fork modules](overlay), and checks the resulting file digests. At registration, a real Flask application registers its views, Werkzeug binds the rules, and Jinja parses the template. The exporter builds a direct-call dispatch function from the registered views, method table and template program. The static server calls MarkupSafe's five-character escaping implementation and uses a frozen itsdangerous-compatible HMAC timestamp signer. The generated source graph for the recorded mixed benchmark is [pinned by hash](mixed-graph.json). The [application](examples/freeze_mixed.py) and [ordinary Python views](examples/mixed_views.py) remain small.

The published mixed workload cycles through three health requests, three HTML greetings with escaping, UTF-8 and repeated query arguments, and three JSON requests containing signed integers up to 160 bits. Separate admissions check error handling, signed-cookie issuance, replay and tampering. A second [catalog application](examples/freeze_catalog.py) exercises different routes, template slots, state and methods, and reads and updates cookies issued by stock Flask. Admission checks that the traced native process owns both listening and accepted sockets and that it maps no libpython.

The frozen API accepts literal paths and a final plain string converter, explicit concrete views, named ordinary-string Jinja slots, and uncompressed integer-valued session dictionaries with one signing key. The transport serves serially, closes each connection, and bounds headers and body sizes. It omits the general WSGI lifecycle, CLI, signal dispatch, middleware, full Jinja language, custom converters, compressed/tagged sessions, key rotation, persistent connections, chunked transfer and streaming responses. An authenticated compressed or noninteger-valued stock cookie opens as empty in this profile. The example key is public benchmark data. The [ordinary WSGI example](examples/mixed_wsgi.py) uses the broader upstream framework at request time.

## Recorded results

The five paired rounds on an AMD EPYC Milan used one server CPU and three separate client CPUs. All 360,000 measured responses passed. The frozen server's median paired throughput ratio to Rust was **1.079×**, ranging from 1.017× to 1.105×. These measurements compare the bounded transport and frozen API to Hyper on this HTTP contract. They include both compilation choices and differences in framework work.

| Server | Median requests/s | Median trial p95 | Median server CPU/request |
| --- | ---: | ---: | ---: |
| Frozen Flask on Imre | 9,650 | 1.459 ms | 78.3 µs |
| Rust / Hyper 1.12 | 9,254 | 1.645 ms | 94.4 µs |

The [recorded samples](results.json) include exact compiler/runtime digests, per-round CPU and latency data, and the preceding short comparison against stock Flask, CPython, Node and Bun. The short comparison used 180 measured requests per round, so use the longer series for the frozen/Rust comparison. The long run used the compiler and runtime built from Imre main at `1cc948ed`. The short comparison used an earlier experimental build.

The upstream notices for the modified software are in [LICENSES.md](LICENSES.md). The parent Flask benchmark retains the original wheel URLs and hashes.
