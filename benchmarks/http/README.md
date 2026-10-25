# HTTP server benchmark

[serve.py](serve.py) runs unchanged on CPython and Imre, using IPv4 TCP sockets and select.poll(). It serves a six-byte GET reply over loopback. TLS, request bodies and general HTTP parsing are outside this benchmark.

On Linux, use the repository's Rust and Clang tools, Python 3.11 or later for the runner, and Go for the load generator. Install Node or Bun if selected; the Rust Hyper and Tokio versions are pinned in [Cargo.lock](rust/Cargo.lock).

    python3 benchmarks/http/verify.py --python python3
    python3 benchmarks/http/run.py --out /tmp/http-small.json
    python3 benchmarks/http/run.py --header-bytes 1024 --clients 16 --out /tmp/http-headers.json

Use `--servers imre,cpython` to compare identical source. Use `selectors` for the idiomatic CPython control (built-in byte searches and blocking writes to fast clients); add `node,bun,rust` for the platform HTTP servers. The identical CPython source and Imre server both handle delayed writes. Select interpreters and toolchains with `--python`, `--node`, `--bun` or `--toolchain ohm`. For Linux CPU affinity, provide `--server-core 2 --client-cores 3,4` using separate cores. The output records revisions, versions, release artifact hashes and each trial.

The driver builds the Imre compiler, runtime and Rust control in Cargo release mode, verifies Cargo's reported optimization level, and compiles the Python endpoint at O3 against that runtime. Check a native binary directly with `python3 benchmarks/http/verify.py /path/to/compiled-serve`. The verifier forces short writes with 2 KiB socket buffers and requires another client to progress while the first has 1,001 unread replies. It also covers idle peers, fragmented headers, a connection reset, response statuses, bodies and EOF.

The defaults are five rotated rounds at 16 and 64 persistent connections, a one-second warmup and 2.5 measured seconds. Each connection sends a request, verifies status, length and body, then sends the next one. All connections must make progress. Failed trials are retained and cause a nonzero exit. Timings exclude build and startup.

Compare medians for throughput and p95, as well as the ranges. Repeat on a quiet machine before interpreting small differences. The default is closed-loop. Pass e.g. `--clients 64 --rate 20000` to schedule requests independently of completions. The driver drains every scheduled request and records delay before dispatch, latency from the scheduled time, and how many replies completed inside the measurement window. Node, Bun and Hyper implement a more general HTTP protocol than this example.
