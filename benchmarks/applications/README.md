# Application benchmarks

These are end-to-end command-line timings on real inputs. Each trial starts a process, reads its inputs and writes its result. The same Python sources run on Imre and CPython. Bun and Rust use separate implementations of the same operations. The suite also times uv’s original script on CPython and graph loading without route queries.

| Workload | Work performed | Correctness check |
| --- | --- | --- |
| uv download catalog | Decompress the pinned python-build-standalone index, select builds from 58,721 artifacts, then write the 6,098-entry download catalog. | Compare every byte of the 3,440,812-byte JSON file with uv’s original implementation. Time that implementation separately as **CPython · uv**. The **CPython · typed** bar runs the Imre input program. |
| Source search | Open the 1,956 tracked Rust files in a pinned Ruff checkout, search for the literal `TODO` and print one `path:line` per matching line. All runtimes read the same sorted manifest. | Check all 542 matches against the output derived from `git grep -n -F TODO`. The timed process includes opening each source file. Preparing the checkout and manifest happens before timing. |
| Wheel verification | Open the real 465-member pip 26.1.1 wheel, parse its RECORD CSV, decompress payloads and check their SHA-256 digests and sizes. Imre and CPython execute PyPA installer’s vendored verifier. Rust uses `zip`, `csv` and `sha2`. Bun uses `fflate`, `csv-parse` and its crypto implementation. | Require success with empty output on the real wheel. Repack one changed byte with a valid ZIP CRC and require each verifier to reject the named member. The ports cover the SHA-256 RECORD format used by this wheel. |
| New York road routing | Load the DIMACS distance graph (264,346 vertices and 733,846 directed arcs), then run Dijkstra for four fixed origin–destination pairs. The **Road graph load** control constructs the same adjacency lists and prints the graph size. | Check distances 556089, 664234, 408822 and 306513. These were independently verified by searching the reversed graph with CPython’s `heapq`. A five-node control also exercises parallel arcs, a shorter alternative path, an unreachable target and a zero-length route. |
| ty conformance report | Read 146 Python files and two captured ty diagnostic runs, classify the changes and write the full offline GitHub report. | Compare all 109,258 bytes against Ruff’s original reporter. A separate unchanged-diagnostics capture must also reproduce the oracle. |

The source search is a literal scan of a warm source tree. The four implementations never invoke an external grep. The routing implementations use adjacency lists and a binary heap without preprocessing. Each stops at the destination and follows its heap’s ordering for equal-cost entries. Bun uses TinyQueue and Rust uses `BinaryHeap`. The shared Python uses the heap from Imre’s existing route-planner example. The conformance comparison exercises the GitHub report mode with its summary table enabled. [uv workload](../../examples/python_catalog/README.md) · [Ruff tree](https://github.com/astral-sh/ruff/tree/370f735ce20e5c79777e773249753e006cb2e04e) · [PyPA verification source](../../examples/wheel_record/README.md) · [Ninth DIMACS Challenge](https://www.diag.uniroma1.it/~challenge9/download.shtml) · [Conformance workload](../../examples/conformance_report/README.md)

## Measurements

Each process inherits affinity to one Linux vCPU. The driver uses warm filesystem caches, stock CPython with its JIT disabled, Imre `-O3`, Cargo release builds and Bun’s ordinary file entry point. Elapsed time includes runtime startup, imports, input and output. Compilation, dependency installation, fixture preparation and output validation are outside the clock. The four implementations receive two warm-ups and 12 measured rounds in balanced order. The catalog has five implementations and 10 measured rounds in forward and reverse rotations. Every measured result is validated.

Before a workload, the driver waits up to one minute for a three-second window with at most 5% activity on the selected vCPU and at most 10% on the other vCPUs. It applies the same check immediately after the workload. During timing, the other vCPUs must average at most 10% busy and the selected vCPU may have at most 1% steal time. A rejected batch is checkpointed with every timing and retried as a unit, up to four attempts. The chart uses the first accepted batch of each workload. The receipt includes the activity windows, individual timings in execution order, source and executable hashes, output hashes and runtime versions. The build manifest identifies the original executables and sources. The runner records its own current SHA-256 separately so those executables can be reused after a driver change.

Median elapsed milliseconds with the inclusive interquartile range in parentheses. The November 2, 2026 UTC run used vCPU 13 on a 32-vCPU AMD EPYC Milan VM. The other CPUs averaged 0.40–0.89% busy during the workloads. The CLI chart uses Imre built from this checkout, based on `fcba9a56`, with clang 18.1.3, Bun 1.4.4, Rust 1.98.1-dev (Ohm commit `f62703110`) and CPython 3.14.5. A second uncached compilation reproduced all five measured Imre executables byte for byte. [All warm-ups, samples, source hashes and activity windows](../results/applications.json)

| Workload | Imre | Bun | Rust | CPython |
| --- | ---: | ---: | ---: | ---: |
| uv download catalog (typed Python) | 152.2 (151.6–154.3) | 401.0 (369.7–453.2) | 104.3 (103.3–104.6) | 2,821.4 (2,808.8–2,851.3) |
| Source search | 165.4 (164.9–166.5) | 149.1 (147.9–152.4) | 76.8 (76.4–77.5) | 188.6 (187.4–194.0) |
| Wheel verification | 37.4 (36.8–38.3) | 265.8 (261.7–277.7) | 27.7 (27.1–28.2) | 145.6 (144.6–147.2) |
| Road graph load | 65.0 (64.5–67.2) | 468.6 (465.1–587.5) | 88.3 (87.5–90.8) | 521.0 (514.4–655.3) |
| Road routing | 116.1 (115.3–118.7) | 848.5 (828.9–877.1) | 140.8 (133.9–199.0) | 1,581.9 (1,263.3–1,620.3) |
| Conformance report | 28.9 (28.5–29.7) | 70.6 (66.7–102.0) | 21.8 (21.7–22.6) | 243.9 (243.4–250.0) |

On the same catalog input, uv’s original script on CPython measured **1,312.5 (1,207.3–1,417.6) ms** and produced the same output as the typed program. The independent HTTP comparison uses Imre 0.1.5. [Frozen Flask measurements](../flask/comparison/README.md)

The suite previously ran on a 16-vCPU Milan host. Its CLI workloads passed the same checks. Its two HTTP rotations exceeded the background-activity limit. On the 32-vCPU host, an earlier version of the driver discarded one catalog batch when the selected vCPU was 14.77% busy immediately afterward. The checked-in driver retains rejected batches. Both README charts use the 32-vCPU host. [Earlier CLI samples](../results/applications-16-vcpu.json.gz) · [Excluded HTTP runs](../flask/comparison/results/2026-11-02-excluded.json.gz)

## Road graph loading and four routes

The previous Imre build and this checkout compiled the same Python source from revision `fcba9a56fa23`. Every process loaded all 733,846 directed arcs from the DIMACS New York file. The routing case also ran four Dijkstra queries in their original order. The five executables rotated through 40 rounds on vCPU 13 with warm filesystem caches. For each accepted batch the other CPUs stayed at or below 2% busy before, during and after timing.

| Median elapsed time (ms) | Previous Imre | Imre | Rust | Imre ThinLTO | Rust ThinLTO |
| --- | ---: | ---: | ---: | ---: | ---: |
| Load graph and count edges | 95.33 | 63.38 | 86.08 | 60.16 | 82.73 |
| Load graph and find four routes | 156.74 | 113.36 | 129.88 | 105.52 | 126.34 |

With the standard builds, the median within-round Imre/Rust routing time ratio was **0.866 (0.851–0.878)**. Parentheses give the 95% paired bootstrap interval. With ThinLTO enabled for both runtimes the ratio was 0.837 (0.825–0.846). After the full CLI suite, a separate 24-round run measured 114.15 ms for Imre and 134.03 ms for Rust, a ratio of 0.868 (0.848–0.877). All builds used the system allocator.

Imre reads short lines from the validated UTF-8 buffer and keeps their bytes in local slots when every use can consume them there. Longer lines keep an owning string. The numeric-field cache recognizes short ASCII decimals and defers other fields to Python’s conversion at the original subscript. The compiler also borrows fixed-size integer-list buffers and allocates private rows in batches. Median peak process memory across six separate runs was 14.2 MiB for Imre, 16.1 MiB for the previous build and 41.6 MiB for Rust.

Each of the other CLI workloads ran in 12 alternating pairs. Ratios below 1 indicate a shorter elapsed time in this checkout.

| Control | Previous Imre (ms) | Imre (ms) | Imre/previous (95% paired interval) |
| --- | ---: | ---: | ---: |
| uv download catalog | 144.8 | 146.0 | 1.004 (1.000–1.013) |
| Source search | 166.4 | 166.2 | 1.007 (0.952–1.019) |
| Wheel verification | 37.2 | 36.3 | 0.998 (0.983–1.012) |
| Conformance report | 29.3 | 28.6 | 0.992 (0.746–1.029) |

[Routing samples, build hashes and correctness checks](../results/road-routing-text.json) · [Previous packed-row measurements](../results/road-routing-packed.json)

## Reproduce on Linux x86-64

Use CPython 3.14.5, Bun 1.4.4, a Rust release toolchain and clang 18. Build the compiler and runtime from this checkout into the same release directory. The tracked uv index and conformance capture ship with the repository. The runner checks the Ruff corpus, pip wheel and uncompressed road graph by SHA-256. [Earlier CLI run using the published Imre 0.1.5 release](../results/applications-release-0.1.5.json)

```sh
# From the repository root, with these executables and an available CPU.
python=/path/to/cpython-3.14.5/bin/python3
export CARGO_TARGET_DIR="$PWD/benchmarks/local/applications/compiler-target"
cargo +ohm -Zohm-defaults=no build --locked --release -p imre-compiler -p imre-runtime
imre="$CARGO_TARGET_DIR/release"
bun=/path/to/bun
cpu=13
dir="$PWD/benchmarks/local/applications"
mkdir -p "$dir"
ruff=370f735ce20e5c79777e773249753e006cb2e04e
curl -fL "https://codeload.github.com/astral-sh/ruff/tar.gz/$ruff" -o "$dir/ruff.tar.gz"
curl -fL 'https://raw.githubusercontent.com/Sanzo00/Bellman-Ford/c6ef5b7be469e0bea411bf78246474ab7fd997b7/data/USA-road-d.NY.gr' -o "$dir/NY.gr"
wheel=$("$python" -c 'from pathlib import Path; import ensurepip; print(next((Path(ensurepip.__file__).parent / "_bundled").glob("pip-26.1.1-*.whl")))')

"$python" benchmarks/applications/build.py --toolchain ohm --imre-prefix "$imre" --bun "$bun" --build-dir "$dir/build"
"$python" benchmarks/applications/run.py --build-dir "$dir/build" --bun "$bun" --cpu "$cpu" \
  --ruff-archive "$dir/ruff.tar.gz" --wheel "$wheel" --graph "$dir/NY.gr" --output "$dir/results.json"
uv run --script benchmarks/render_runtime_suite.py "$dir/results.json" --output-dir "$dir/charts"
```

To use the measured Ohm compiler, pass `--toolchain ohm` to the build step and set `CARGO_BUILD_BUILD_DIR` to your shared Ohm build cache. The build script uses `cargo +ohm -Zohm-defaults=no` in that configuration. The uncompressed graph mirror has SHA-256 `37f5b3eff82330850466956b6c036a8ab8551512bdc7509edca1d4c6cf943fa3`. The original DIMACS dataset is named `USA-road-d.NY.gr`.
