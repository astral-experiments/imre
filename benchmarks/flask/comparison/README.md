# HTTP performance of the unchanged Flask app

A standalone Imre timing requires the unchanged Flask app and its seven pinned wheels to serve all ten HTTP cases from the same Linux ELF. On the timing host, the harness compares response fingerprints against CPython, checks the original view exception and verifies the listener and accepted socket owners with procfs and `strace`. It follows workers and checks for a foreign executable or a mapped `libpython`. Each timing process must match the proven SHA-256 before warmup. [Ten requests and expected responses](../README.md#flask-over-tcp)

The published eight-client mix ran on one AMD EPYC Milan server vCPU. CPython/Flask served 934 requests/s, Node 3,379, Bun 8,379 and Rust/Hyper 9,029. The original standalone Imre candidate exited before listening and has no throughput or latency measurement. Repeat all four controls with a new Imre candidate on the same host and reserved CPUs. [Measured rates, latency percentiles and individual trials](RESULTS.md#flask-node-bun-and-rust)

## Workload

Every cycle interleaves three requests to each route. CPython runs the checked-in Flask app through Werkzeug and renders its greeting with Jinja. Node and Bun share JavaScript handlers, and the Rust control uses Hyper with single-thread Tokio and arbitrary-precision JSON. Those three controls implement the routes and HTML escaping directly.

| Route | Work performed |
| --- | --- |
| `GET /health` | Serialize a JSON boolean. |
| `GET /hello/<name>` | Decode a path and query, select the first suffix, escape both fields and return UTF-8 HTML. Cases include quotes, HTML metacharacters, a literal plus and Japanese text. |
| `POST /total` | Parse and sum 128 small integers, a 96-bit integer with a one-unit residual, or two 160-bit integers that cancel with a negative result. |

The timed client checks the HTTP status, content type and exact body bytes on every response. Before timing, each server must also pass the existing five TCP oracles, including the 404, a missing JSON field that produces 500, and a subsequent 200 from the same process. The alternate HTTP controls serve Flask's recorded error pages. Separate Flask session profiles request `/visits` with a fresh cookie jar every time or replay each previous signed cookie and check the increasing count.

Each server runs alone on one Linux vCPU. The Go closed-loop client occupies three other vCPUs, sends HTTP/1.1 with `Connection: close`, and opens a new loopback TCP connection per request. The wire control requires EOF after a complete response. Each round starts a fresh server process, shuffles the profiles, and warms each profile with 252 verified requests before measuring 2,016. The harness rotates and reverses server order between rounds. It measures aggregate throughput and request-to-complete-body latency including connection setup. It records per-round p50, p95 and p99, client/server CPU time and other activity on the assigned cores. These latency percentiles describe a closed-loop load with one or eight clients.

## Reproduce

From the repository root on Linux x86-64, run the ten TCP checks on CPython 3.14.5 using the locked wheels.

```sh
bench=benchmarks/flask/comparison
python3.14 -m pip download --no-deps --require-hashes --dest /tmp/flask-wheels \
  -r benchmarks/flask/requirements.txt
python3.14 benchmarks/flask/gate.py --wheelhouse /tmp/flask-wheels \
  --reference-only --output /tmp/flask-reference.json
```

For the cross-language comparison, build the release Imre runtime and both native control programs before timing. Node and Bun must support the JSON reviver's `context.source` to preserve the original integer tokens. Set `SERVER_CPU` and `CLIENT_CPUS` to disjoint CPUs available on the timing host. The defaults below reproduce the published core assignment.

```sh
server_cpu=${SERVER_CPU:-28}
client_cpus=${CLIENT_CPUS:-29,30,31}
go build -o /tmp/flask-client "$bench/client.go"
cargo build --manifest-path "$bench/rust/Cargo.toml" --target-dir "$bench/rust/target" --release --locked
cargo build --target-dir target --release --workspace --locked
python3.14 "$bench/compare.py" --wheelhouse /tmp/flask-wheels \
  --client /tmp/flask-client --rust "$bench/rust/target/release/imre-flask-hyper-control" \
  --node "$(command -v node)" --bun "$(command -v bun)" \
  --compiler target/release/imre --runtime target/release/libimre_runtime.a \
  --runtime-imports --build-timeout 420 --rounds 8 \
  --server-cpu "$server_cpu" --client-cpus "$client_cpus" --output /tmp/flask-comparison.json
python3 "$bench/summary.py" /tmp/flask-comparison.json
```

The comparison builds `serve.py` at O3. The example allows 420 seconds for compilation and records the budget and elapsed time in `imre_build`. To reuse a release-linked ELF from this same app and wheel set, replace `--compiler`, `--runtime` and `--runtime-imports` with `--imre-binary /absolute/path/to/server`. Both forms run the native proof before timing. The JSON records the proof under `imre_native_preflight` and saves the raw trace beside it. A failed build or proof leaves Imre out of `trials`, while eligible controls can still run. Use `--rounds 0` to run only the preflights, or `--mixed-only --concurrency 8` for the mixed workload alone.

The negative control supplies the Rust server as the Imre candidate. It serves the health and big-integer JSON responses but fails the signed session check. The command verifies that the control servers receive timing rows and Imre receives none.

```sh
python3.14 "$bench/check_native_negative.py" --wheelhouse /tmp/flask-wheels \
  --client /tmp/flask-client --rust "$bench/rust/target/release/imre-flask-hyper-control" \
  --node "$(command -v node)" --bun "$(command -v bun)" \
  --server-cpu "$server_cpu" --client-cpus "$client_cpus"
```

## CPython WSGI servers

To compare Werkzeug with Waitress on the original Flask app, add the Waitress 3.0.2 wheel. The runner verifies its SHA-256 and pins its single application worker and I/O loop to the server CPU.

```sh
python3.14 -m pip download --no-deps --dest /tmp/flask-wheels waitress==3.0.2
python3.14 "$bench/compare.py" --wheelhouse /tmp/flask-wheels --client /tmp/flask-client \
  --wsgi-pair --rounds 9 --server-cpu "$server_cpu" --client-cpus "$client_cpus" \
  --output /tmp/flask-paired.json
```

An October 2026 experiment ran LLVM-compiled views inside CPython. Its timings measure a CPython process serving Flask and Werkzeug. [Historical hosted measurements](RESULTS.md#same-flask-app-with-different-execution-and-serving-configurations)
