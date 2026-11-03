# Application benchmarks

Each CLI starts a fresh process, reads its inputs and writes its result. Imre and CPython run the same Python source. The Rust references implement the same workloads.

| Workload | Input and output checked on every timed run |
| --- | --- |
| uv download catalog | Select builds from 58,721 release artifacts. Compare all 3,440,812 bytes of the catalog with uv’s original Python script, which is also timed separately. |
| Source search | Read 1,956 Rust files from Ruff and print the line numbers containing `TODO`. Compare all 542 matches with `git grep -n -F`. |
| Wheel verification | Open the 465-member pip 26.1.1 wheel and verify member sizes and SHA-256 hashes against RECORD. Before timing, the verifier must identify a deliberately damaged member. The Python program calls PyPA installer. |
| Road routing | Read the New York DIMACS graph with 264,346 nodes and 733,846 directed arcs. Check the four shortest-path distances against an independent reverse-graph search. A separate run checks the graph size and measures loading alone. |
| ty conformance report | Read 146 Python files and two captured diagnostic runs. Compare the full 109,258-byte GitHub report with Ruff’s original reporter, and check an unchanged-diagnostics control before timing. |

## Results

Median elapsed milliseconds on one AMD EPYC Milan vCPU with warm filesystem caches. The runner rotates execution order, includes process startup and checks the full output after each run. [Samples, build hashes and host activity](../results/applications.json)

| Workload | Imre | Rust | CPython |
| --- | ---: | ---: | ---: |
| uv download catalog | 153.8 | 104.3 | 2,801.5 |
| Source search | 53.4 | 46.2 | 184.7 |
| Wheel verification | unsupported | 25.5 | 149.1 |
| Road graph load | 60.5 | 60.1 | 526.9 |
| Road routing | 103.1 | 87.1 | 1,525.9 |
| Conformance report | 29.5 | 21.1 | 252.8 |

uv’s original script on CPython took 1,157.5 ms. During the accepted CLI runs, the other 31 vCPUs averaged 0.50–1.70% busy.

In 24 paired rounds, skipping nonmatching split fields and sizing regular-file reads cut Imre’s source search from 163.1 to 54.1 ms. Searching whole files and counting newlines between hits cut Rust from 71.1 to 47.5 ms. In 20 routing rounds, Rust took 126.3 ms with per-node vectors, 103.3 ms with a flat edge table and 84.9 ms with rows of up to four packed edges stored inline. Longer rows and wider values use overflow storage. Imre took 101.0 ms on the same graph. The 95% bootstrap interval for the median paired difference (Imre minus Rust) was 15.0 to 17.5 ms. [Paired samples](../results/application-pairs.json.gz)

In a separate Callgrind run, loading the graph took 464 million instructions in Imre and 460 million in Rust. Including the four routes brought those totals to 845 million and 581 million.

The compiler and runtime come from `3bdf94075`. Rust uses release builds. The catalog, router and conformance reporter use ThinLTO and one codegen unit. The Imre router links its runtime with ThinLTO. Imre parses integer fields directly from the input text and stores rows of up to four integer pairs inline.

On this compiler, PyPA installer’s runtime-selected `hashlib.new(name)` takes the object-runtime path and fails to import `zipfile`. The Imre wheel verifier previously took 37.4 ms and rejected the damaged wheel. [Last working samples](https://github.com/astral-experiments/imre/blob/1a6d68fc8d6f3f2598bb9af658483c0415e7aa91/benchmarks/results/applications.json)

## Flask

Eight clients issue an equal mix of health checks, escaped HTML greetings and JSON sums over loopback. Each round warms up with 180 requests, then checks all 1,800 timed responses. Frozen Flask on Imre 0.1.5 registers routes and compiles its Jinja template ahead of time. CPython runs stock Flask with Werkzeug. Rust implements the routes directly with single-threaded Hyper.

Median throughput was 9,521 requests/s for Imre, 9,558 for Rust and 1,028 for CPython. The HTTP run used the Flask suite’s 10% peer-CPU limit. All seven ten-round runs met that limit. The other 28 vCPUs averaged 6.46–9.78% busy during traffic. The host security monitor accounted for about two cores. [HTTP samples and native-server checks](../flask/comparison/results/2026-11-03-frozen-mixed.json.gz)

## Reproduce

From the repository root on Linux, use CPython 3.14.5, clang 18 and an available CPU. The published run used Rust 1.99.0-dev (Ohm). Set `CARGO_BUILD_BUILD_DIR` to a separate shared Ohm build directory.

```sh
python=/path/to/python3.14
dir=$(mktemp -d)
export CARGO_TARGET_DIR="$dir/compiler"
target=$(rustc +ohm --print host-tuple)
lld=$(rustc +ohm --print sysroot)/lib/rustlib/$target/bin/gcc-ld/ld.lld
cargo +ohm -Zohm-defaults=no build --locked --release -p imre-compiler -p imre-runtime
RUSTFLAGS=-Clinker-plugin-lto cargo +ohm -Zohm-defaults=no build --locked --release \
  --target-dir "$dir/bitcode" --target "$target" -p imre-runtime
"$python" benchmarks/applications/build.py --build-dir "$dir/build" --toolchain ohm \
  --imre-prefix "$CARGO_TARGET_DIR/release" --lto-linker "$lld" \
  --lto-runtime "$dir/bitcode/$target/release/libimre_runtime.a"

curl -fL 'https://codeload.github.com/astral-sh/ruff/tar.gz/370f735ce20e5c79777e773249753e006cb2e04e' -o "$dir/ruff.tar.gz"
curl -fL 'https://raw.githubusercontent.com/Sanzo00/Bellman-Ford/c6ef5b7be469e0bea411bf78246474ab7fd997b7/data/USA-road-d.NY.gr' -o "$dir/NY.gr"
wheel=$("$python" -c 'import ensurepip, pathlib; print(next(pathlib.Path(ensurepip.__file__).parent.glob("_bundled/pip-26.1.1-*.whl")))')
"$python" benchmarks/applications/run.py --cpu 13 --build-dir "$dir/build" \
  --ruff-archive "$dir/ruff.tar.gz" --graph "$dir/NY.gr" --wheel "$wheel" --output "$dir/results.json"
uv run --script benchmarks/render_runtime_suite.py "$dir/results.json" --output-dir "$dir/charts" \
  --flask-results benchmarks/flask/comparison/results/2026-11-03-frozen-mixed.json.gz
```

For one ten-round HTTP run, set `release` to the extracted x86-64 Linux release directory. Prepare the frozen graph from `fcba9a56`, then build the Go client and Rust server from this checkout. [Imre 0.1.5 release](https://github.com/astral-experiments/imre/releases/tag/0.1.5)

```sh
mkdir "$dir/flask-src"
git archive fcba9a56fa23067ca33e2ed3a23af6cd66e7d2e8 | tar -x -C "$dir/flask-src"
"$python" -IB "$dir/flask-src/benchmarks/flask/frozen/prepare.py" "$dir/frozen" \
  --imre "$release/imre" --runtime "$release/libimre_runtime.a" --clang clang-18
go build -o "$dir/client" benchmarks/flask/comparison/client.go
cargo +ohm -Zohm-defaults=no build --manifest-path benchmarks/flask/comparison/rust/Cargo.toml \
  --locked --release --config 'profile.release.lto="thin"' --config profile.release.codegen-units=1
FLASK_SECRET_KEY=local-probe-key "$python" -IB benchmarks/flask/comparison/compare.py \
  --imre-binary "$dir/frozen/mixed/server-O3" --wheelhouse "$dir/frozen/wheels" \
  --client "$dir/client" --rust "$CARGO_TARGET_DIR/release/imre-flask-hyper-control" \
  --mixed-only --concurrency 8 --rounds 10 --warmup 180 --requests 1800 \
  --server-cpu 13 --client-cpus 8,9,10 --output "$dir/flask.json"
```
