# Flask measurements on October 26, 2026

## Flask, Node, Bun and Rust

On one AMD EPYC Milan vCPU, the unchanged Flask application through Werkzeug served 934 requests/s with eight concurrent clients. The Node, Bun and Rust route implementations reached 3,379, 8,379 and 9,029 requests/s. Each mixed trial contained equal numbers of JSON health checks, escaped greetings and exact integer sums. Rates and latencies below are medians of eight warmed trials. [Per-round observations and controls](results/2026-10-26-controls.json.gz)

| Server | Mixed, 1 client, req/s | Mixed, 8 clients, req/s | 8 clients, p50 ms | p95 ms | p99 ms | Min–max req/s at 8 clients |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| CPython 3.14.5, Flask 3.1.3, Werkzeug 3.1.8 | 804 | 934 | 8.46 | 10.17 | 10.90 | 809–979 |
| Node 24.22.1 | 1,594 | 3,379 | 2.10 | 4.39 | 7.87 | 2,629–3,982 |
| Bun 1.4.2 | 1,799 | 8,379 | 0.78 | 1.81 | 3.64 | 7,705–8,925 |
| Rust, Hyper 1.12.0 | 2,145 | 9,029 | 0.73 | 1.62 | 3.71 | 7,862–9,724 |

The Bun and Rust throughput ranges overlap. In the eight-client mix the median Go client used 2.17 of three vCPUs for Bun and 2.27 for Rust. These native HTTP controls implement the measured endpoints directly. The Flask implementation runs its routing, request lifecycle and Jinja template machinery.

| Single-route workloads, 8 clients | Health req/s | Health p95 ms | Greeting req/s | Greeting p95 ms | JSON sum req/s | JSON sum p95 ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| CPython / Werkzeug | 1,258 | 6.90 | 648 | 13.12 | 1,151 | 7.90 |
| Node | 3,794 | 3.01 | 3,689 | 3.57 | 2,891 | 3.84 |
| Bun | 9,393 | 1.53 | 9,600 | 1.55 | 7,056 | 1.79 |
| Rust / Hyper | 9,798 | 1.54 | 9,401 | 1.60 | 8,436 | 1.68 |

The fully native Imre O3 build at `7da66d7f` stopped at `werkzeug/urls.py:54:25` on a starred element in a module-level tuple. That build served zero requests. The timed controls passed 354,816 measured responses plus warmups; one complete round was repeated in the same order after a separate preflight briefly used the server CPU. Both the excluded round and the replacement provenance are in the receipt.

## Same Flask app with different execution and serving configurations

Nine further rounds interleaved three configurations of the original application with a shared CPython/Werkzeug control. The hosted experiment compiles the unchanged views to LLVM, runs them on CPython objects, and leaves Flask, Jinja, JSON, sessions and Werkzeug on CPython. Waitress 3.0.2 uses one application worker plus its I/O loop, all pinned to the same server vCPU. Werkzeug responded with HTTP/1.0 and Waitress with HTTP/1.1. Every request in all three configurations required a fresh TCP connection. [Paired per-round observations and artifact hashes](results/2026-10-26-wsgi-paired.json.gz)

| 8-client mixed workload | Requests/s | p95 ms | p99 ms | Within-round rate / CPython, median (range) |
| --- | ---: | ---: | ---: | ---: |
| CPython / Werkzeug | 917 | 9.99 | 11.05 | 1.000 |
| Hosted LLVM views / Werkzeug | 901 | 10.13 | 10.61 | 0.983 (0.949–1.143) |
| CPython / Waitress | 812 | 13.28 | 16.03 | 0.883 (0.781–0.968) |

| Median requests/s | CPython / Werkzeug | Hosted LLVM / Werkzeug | CPython / Waitress |
| --- | ---: | ---: | ---: |
| Health, 8 clients | 1,257 | 1,214 | 1,305 |
| Greeting, 8 clients | 654 | 644 | 464 |
| JSON sum, 8 clients | 1,126 | 1,116 | 1,108 |
| Equal mix, 1 client | 809 | 704 | 872 |
| Fresh signed session, 1 client | 918 | 901 | 984 |
| Replayed signed session, 1 client | 843 | 824 | 917 |

Hosted views were faster than ordinary CPython in three of nine eight-client mixed pairs. Waitress served the single-client mix faster in eight of nine pairs and the eight-client mix slower in all nine. The closed-loop workload, fresh connections and one worker define the comparison. The median other activity on the server core was 0%, with a 95th percentile of 0.61% across these trials.

We repeated the eight-client mix in six alternating pairs on the published hosted adapter, which preserves the original view source line in a Flask traceback. The extension SHA-256 starts with `be35117d08bd`; the launcher starts with `5d7103a22e44`. This artifact served **810 requests/s** with p50/p95/p99 of **8.80/10.57/31.11 ms**. Its contemporaneous ordinary CPython control served **840 requests/s** at **8.55/10.54/25.58 ms**. The median within-round ratio was **0.955**, ranging from 0.832 to 1.149. Hosted LLVM was faster in two of six pairs. All 24,192 timed responses were correct. These measurements show the original application serving over real TCP with LLVM-compiled views, with no demonstrated throughput gain over ordinary CPython. [Published-adapter paired observations](results/2026-10-26-hosted-published.json.gz)

## Where the Flask server spends CPU

A separate deterministic profile followed the single Werkzeug thread's CPU clock while it served 900 verified mixed requests after warmup. All 300 greetings compiled a Jinja template in both modes. Each percentage is cumulative for that call tree and the trees overlap. The denominator is 6.203 profiled thread CPU seconds for CPython or 5.690 for hosted LLVM. The profiler instruments calls; the independent warmed trials above measure throughput. [Function-level CPU profiles](results/2026-10-26-cpu-profile.json.gz)

| Call | Calls in each mode | CPython share | Hosted LLVM share |
| --- | ---: | ---: | ---: |
| Werkzeug request handler `run_wsgi` | 900 | 82.0% | 81.2% |
| Flask `wsgi_app` | 900 | 57.9% | 56.4% |
| Flask `render_template_string` | 300 | 22.7% | 23.6% |
| Jinja `Environment.compile` | 300 | 20.1% | 20.8% |

Reproduce the profile with `profile_cpu.py --wheelhouse WHEELS --client CLIENT --hosted-launcher LAUNCHER --output-dir PROFILE`. It runs separately from throughput measurement on disjoint cores. The full comparison methodology and build commands are in the [benchmark instructions](README.md).
