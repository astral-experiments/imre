> Project naming is normalized to Relar; these are historical measurements. See [capture provenance and exact restoration](NAMING_PROVENANCE.md).

# Route planner: first AOT result

On 2026-10-17, the compiled route planner ran 2.17× faster than CPython 3.14.5 on the largest measured workload. Both executed the unchanged [`examples/route_planner.py`](../../examples/route_planner.py), including terrain generation, Dijkstra's algorithm, heap operations, path reconstruction, path validation, and output.

Each invocation solves three routes, using seeds 42, 43, and 44. Times are end-to-end medians from eleven alternating paired samples after two warmup pairs. Every sample starts a fresh process. Parentheses show the observed minimum–maximum, not a confidence interval.

| Grid | CPython median (range), ms | Relar median (range), ms | Ratio of medians |
| --- | ---: | ---: | ---: |
| 32 × 32 | 23.08 (22.68–24.06) | 5.82 (5.59–6.01) | 3.97× |
| 128 × 128 | 108.21 (107.58–112.13) | 47.27 (46.76–48.18) | 2.29× |
| 256 × 256 | 415.27 (412.17–442.57) | 191.28 (189.71–194.96) | 2.17× |

The source-to-executable AOT compilation took **0.515 seconds**, measured separately with the compiler and Rust runtime already built. The native executable is **2,388,656 bytes** and the generated LLVM IR is 63,490 bytes. The executable links system libraries and the independent Rust runtime; it has no CPython dependency. A separate run with an empty environment and no Python on `PATH` also succeeded.

The host reports an AMD EPYC-Milan CPU, x86-64 Linux, and kernel `6.8.0-146-generic`. The harness and children were pinned to CPU 0, outside the process-tracing sandbox. Other local compiler builds and review experiments had finished. This was a shared devbox, without exclusive CPU ownership or fixed frequency controls.

CPython was a non-debug, GIL-enabled 3.14.5 build. Its configuration and compiler flags report PGO and LTO; its JIT was available but disabled. Relar used Ruff/ty 0.0.14 and Clang 18.1.3 at `-O3`. The compiler and runtime were built with `cargo +ohm -Zohm-defaults=no build --workspace --release --locked`, using Ohm `1.98.1-1`, Rust `1.98.1-dev` (`f6270311094cd4b48fefce03debdffcf8396c64c`), and Rust's LLVM 22.1.8 backend. This Rust backend version is separate from the LLVM 18 backend compiling generated Python-program IR.

Before timing, 22 input cases covering 64 routes matched CPython's complete output and an independent Bellman-Ford shortest-path oracle. These include one-cell grids and seeds beyond native integer range. Every warmup and measured run also matched complete output. The same 64-route validation passed with generated code and the Rust runtime crate instrumented by AddressSanitizer, with LeakSanitizer enabled. The compiler's separate differential suite passed 76 checks at O0/O3 and 54 sanitizer checks at O1, including 260 integer pairs and 32 rejection cases.

The measured source and harness revision is [`312557ad1cfc94fcff05741f328c2596273a3f2c`](https://github.com/viarius-experiments/relar/commit/312557ad1cfc94fcff05741f328c2596273a3f2c); the worktree was clean at measurement start. The [raw JSON](2026-10-17-route-planner.json) records every duration and output, input text, hashes, tool versions, compilation time, clock properties, and interquartile ranges. The subsequent commit adds this report and the unmodified result data.

To reproduce, build the workspace and follow the [benchmark instructions](../README.md). The measured run used the defaults: widths 32/128/256, three routes, seed 42, eleven samples, two warmup pairs, and CPU 0. Select CPython 3.14.5 explicitly with `--python` to reproduce the baseline version.

These results establish a working Rust/LLVM AOT path for this typed first-party program. They measure startup and process I/O as well as algorithm execution; the small-grid ratio includes a larger startup contribution. They do not establish a general Python speedup or extension compatibility. The enforced subset, source restrictions, and ownership model are documented in the [prototype contract](../../docs/subset.md).
