# HTTP performance of the unchanged Flask app

The reference serves the checked-in `tiny_flask_app.py` through its existing Werkzeug entry point. Node and Bun run the same JavaScript handlers through their own HTTP APIs. The Rust control uses Hyper, single-thread Tokio, arbitrary-precision JSON and `BigInt`. The cross-language controls implement the three measured routes and error responses. Flask runs Jinja for each greeting. The other controls apply HTML escaping directly.

The [measured results](RESULTS.md) include per-route throughput, latency percentiles, the CPython-hosted LLVM comparison, the fully native compiler outcome and a serving-thread CPU profile.

## Workload

Every cycle sends three requests to each route in an interleaved order.

| Route | Work performed |
| --- | --- |
| `GET /health` | Serialize a JSON boolean. |
| `GET /hello/<name>` | Decode a path and query, select the first suffix, escape both fields and return UTF-8 HTML. Cases include quotes, HTML metacharacters, a literal plus and Japanese text. |
| `POST /total` | Parse and sum 128 small integers, a 96-bit integer with a one-unit residual, or two 160-bit integers that cancel with a negative result. |

The timed client checks the HTTP status, content type and exact body bytes on every response. Before timing, each server must also pass the existing five TCP oracles, including the 404, a missing JSON field that produces 500, and a subsequent 200 from the same process. The alternate HTTP controls serve Flask's recorded error pages. Separate Flask session profiles request `/visits` with a fresh cookie jar every time or replay each previous signed cookie and check the increasing count.

Each server runs alone on one Linux vCPU. The Go closed-loop client occupies three other vCPUs, sends HTTP/1.1 with `Connection: close`, and opens a new loopback TCP connection per request. The wire control requires EOF after a complete response. Each round starts a fresh server process, shuffles the profiles, and warms each profile with 252 verified requests before measuring 2,016. The harness rotates and reverses server order between rounds. It measures aggregate throughput and request-to-complete-body latency including connection setup. It records per-round p50, p95 and p99, client/server CPU time and other activity on the assigned cores. These latency percentiles describe a closed-loop load with one or eight clients.

## Reproduce

Use the seven release wheels and CPython 3.14.5 from the parent gate. Node and Bun must support the JSON reviver's `context.source` to preserve the original integer tokens. The comparison accepts the built Go client and Rust server as arguments, so building or installing them is outside the measurement. Choose disjoint Linux cores available to the benchmark process.

```sh
bench=benchmarks/flask/comparison
go build -o /tmp/flask-client "$bench/client.go"
cargo build --manifest-path "$bench/rust/Cargo.toml" --release --locked
python3.14 "$bench/compare.py" --wheelhouse /tmp/flask-wheels \
  --client /tmp/flask-client --rust "$bench/rust/target/release/imre-flask-hyper-control" \
  --node "$(command -v node)" --bun "$(command -v bun)" \
  --compiler target/release/imre --runtime target/release/libimre_runtime.a \
  --server-cpu 28 --client-cpus 29,30,31 --output /tmp/flask-comparison.json
python3 "$bench/summary.py" /tmp/flask-comparison.json
```

The optional Imre row builds the unchanged `serve.py` at O3 against the same wheel tree and must pass the TCP controls to be timed. The JSON records the compiler diagnostic if the app cannot start. Use the parent compatibility gate to run all four build modes and check the Flask traceback.

For a paired WSGI comparison, provide the unmodified Waitress 3.0.2 wheel in the same wheelhouse. The runner checks its SHA-256, extracts it alongside the existing releases and runs the original Flask app with one Waitress application worker. The CPython-hosted LLVM mode accepts a Python launcher whose arguments are the staged wheel directory and the ready file. The launcher imports the compiled views and uses CPython to run the framework and HTTP transport. Include one `--hosted-artifact` per binary or source that the launcher consumes. The WSGI and hosted options can run together and share one contemporaneous CPython/Werkzeug control.

```sh
python3.14 -m pip download --no-deps --dest /tmp/flask-wheels waitress==3.0.2
python3.14 "$bench/compare.py" --wheelhouse /tmp/flask-wheels --client /tmp/flask-client \
  --wsgi-pair --hosted-launcher /path/to/launch.py --hosted-artifact /path/to/_imre_views.so \
  --rounds 9 --server-cpu 28 --client-cpus 29,30,31 --output /tmp/flask-paired.json
```

Use `--rounds 0` for a correctness run. A paired run only needs its own launcher and server dependencies.
