# SQLite issue tracker

Two users keep private issue queues. Each user logs in, creates issues, closes or reopens them, and reads the latest 50 issues by status. Stock Flask on CPython and frozen Flask on Imre share the [credential checks, issue model and SQL](domain.py) and [Jinja templates](presentation.py).

## Run

From an Imre checkout on Linux x86-64, use CPython 3.14.5 with its `sqlite3` module, the repository's Rust toolchain, Clang 18, a C archiver, SQLite 3.35 or newer with development headers, GNU patch, `taskset`, `strace` and `readelf`. The host must allow child-process tracing and provide at least two CPUs. On Debian/Ubuntu the SQLite development package is `libsqlite3-dev`.

```sh
python3.14 benchmarks/flask/tracker/run.py
```

Add `--smoke` for the complete correctness and storage checks plus one short timed trial per server. The runner builds the compiler and runtime from the checkout, verifies seven [pinned upstream wheels](../requirements.txt), and builds the native tracker at O3. Build artifacts use `CARGO_TARGET_DIR` or the checkout’s `target/`. Use `--wheelhouse DIRECTORY` for an offline wheel cache, `--work NEW_DIRECTORY` to preserve all outputs at a chosen path, and `--server-cpu N --client-cpus I,J` to select CPUs. The runner prints the output directory and writes the graph, LLVM, ELF, trace, build hashes and trial receipts there.

## Application

| Request | Effect |
| --- | --- |
| `POST /login` | Verify a username and high-entropy API token. Return a signed HttpOnly session cookie and a fresh CSRF nonce. |
| `POST /api/issues` | Accept a title, optional body and session CSRF token. Commit the issue and return its generated ID. |
| `POST /api/issues/<id>` | Set status to `open` or `closed` when both the CSRF token and row owner match. |
| `GET /api/issues?status=closed` | Return the caller’s newest matching issues as JSON. Omitting the filter selects both statuses. |
| `GET /issues`, `GET /issues/<id>` | Render an autoescaped HTML list or detail page. Private reads vary on the session cookie. |

The runner provisions fresh random credentials. SQLite stores their SHA-256 hashes. A successful login uses OS entropy for the session nonce. Both paths use parameterized SQL and bind the authenticated user ID in reads and updates. A missing or foreign issue returns 404. The [stock views](stock.py) use Flask request and session proxies, `jsonify`, Jinja and CPython’s `sqlite3`. The [frozen views](frozen_views.py) accept concrete requests and state, encode typed JSON records, and call the same domain functions.

The fork exports explicit application modules beside directly dispatched views. It parses one Jinja row loop at registration and emits a renderer that reads named fields from string mappings and escapes them through MarkupSafe. Filters, nested loops and arbitrary attribute expressions fail registration. The SQLite extension links the host library through Imre’s public bytes ABI. It runs one prepared statement in a transaction per connection, binds positional text parameters and returns integer or UTF-8 text columns. Auth, SQL and routing stay in Python. The native backend covers this adapter contract, a subset of the DB-API. [Registration](freeze.py) · [Jinja compiler](../frozen/overlay/jinja2/frozen_rows.py) · [SQLite adapter](storage/tracker_db.py) · [C binding](storage/sqlite.c)

Both transports serve loopback requests serially and close each connection. The tracker uses two provisioned accounts, an owner-only access model, integer-valued session cookies and JSON credential exchange. The frozen signer supports one key and uncompressed payloads. Account management, TLS, key rotation, revocation, full WSGI middleware and unrestricted Jinja are outside this benchmark. [Framework profile and licenses](../frozen/README.md)

## Correctness and timing

The 28-step admission scenario compares HTTP status, media, cookie issuance, exact HTML bytes, private-read `Vary` and decoded JSON. Each implementation rejects tampered cookies, missing or mismatched CSRF tokens and another user's private reads and writes. A second process reuses the cookie and reads a committed issue containing the caller’s Unicode and HTML-sensitive text. It reopens the issue, exits, and an independent reader verifies owner, content and status in SQLite. The separate storage test writes through each backend and reads both databases with fresh processes. It covers embedded NULs, injection-shaped text and rollback on failed constraints and unsupported column types. [Traffic](traffic.py) · [Storage tests](storage/contract.py)

Native admission verifies that the generated ELF owns the listener and an accepted connection. The syscall trace covers a real login, issue insertion and detail read, with SQLite mapped in the server and zero libpython mappings. Timings use fresh untraced processes. Each starts from the same seed DB on the same filesystem, with four clients, one warmup issue cycle per client and 100 measured cycles per client. Each cycle creates an issue, loads its HTML detail, updates its status, filters JSON and renders the filtered HTML list. The runner validates every response and compares both owners’ final 50-row pages to SQLite. Server order alternates by round. Duration includes client validation, SQLite connections, commits and new TCP connections.

## Recorded run

Five rounds on one AMD EPYC Milan server CPU and two client CPUs exercised 10,000 timed requests per implementation. Each database finished with the expected 404 issues, including warmups. The median within-round frozen/stock throughput ratio was **1.179×**, with individual ratios from **1.007× to 1.319×**. The host used ext4 and SQLite 3.45.1. Filesystem commit latency affects these rates.

| Server | Median requests/s | Median trial p95 | Median server CPU/request |
| --- | ---: | ---: | ---: |
| Stock Flask 3.1.3 on CPython 3.14.5 | 151.5 | 54.72 ms | 1,850 µs |
| Frozen Flask on Imre O3 | 173.0 | 46.11 ms | 880 µs |

The public compiler/runtime pair came from `1cc948ed`. The emitted LLVM had zero `imre_dyn_*` call sites and one call site through the public extension ABI for all SQLite statements. The measured graph hashes, executable identities, CPU seconds, individual trials and per-operation p50/p95 values are in the [recorded results](results.json).
