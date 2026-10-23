# Relar

An ahead-of-time compiler for typed Python, written in Rust.

> [!WARNING]
> This README includes human edits; all code changes, PR summaries, and additional
> documentation were authored entirely by GPT-6 Astra in [Codex](https://openai.com/codex/).
> Use at your own risk.

## Language support

Relar compiles a restricted subset of Python into native executables that run
without a Python installation.

Relar supports:

- Statically resolved imports
- Functions with exact declared or inferred signatures
- Classes and dataclasses with fixed fields
- String enums with named members and aliases, and string-valued `Literal` annotations
- Typed containers and arbitrary-precision integers
- Common control flow, comprehensions, and immediately consumed iterator pipelines
- Selected file and path operations
- Decode external JSON and TOML into fixed records and typed containers, and serialize JSON layouts directly.
- Sort lists stably in place or into a new list. Use immutable tuple keys, Unicode text operations, fixed regular expressions, and bounded URL decoding.
- Read and write UTF-8 files, resolve native `pathlib.Path` values, and scan directories with `iterdir` and bounded `glob`.
- Stream lines from plain or gzip files and read binary inputs in bounded chunks.
- Create scoped temporary workspaces and atomically replace completed files.
- Run native subprocesses with captured UTF-8 output, deadlines, explicit arguments, working directories, and environment snapshots.

But not:

- Most of the Python standard library
- Dynamically typed storage (`Any` or unrestricted unions)
- Recursive calls or closures that capture variables
- Reflection or dynamic attributes
- Generator functions or async code
- Arbitrary decorators

## Design

Types are part of Relar's compilation contract: variables, containers, and class
fields have known types, and each function has one declared or inferred
signature. This lets Relar choose concrete representations. It aims to preserve
Python's behavior for supported operations, including arbitrary-precision integers.

Relar's runtime uses reference counting and restricts object graphs to avoid
cycles. Compiled programs run without a Python installation, but Relar must
implement the features and libraries they use. It cannot use arbitrary Python
packages or CPython extensions.

Other projects make different trade-offs:

- [PyPy](https://pypy.org/) optimizes dynamic Python with a JIT as the program
  runs. It supports much more of Python, but needs time to warm up.
- [Codon](https://docs.exaloop.io/) also compiles Python ahead of time. It can
  specialize functions for different argument types and can call CPython
  explicitly. Its default `int` is 64-bit.
- [Static Python](https://facebookincubator.github.io/cinderx/static-python) and
  [mypyc](https://mypyc.readthedocs.io/) use types to optimize code running
  within CPython, so compiled modules can interact with ordinary Python code.

For more, see the [design](docs/design.md) and [supported subset](docs/subset.md).

## Benchmark

Running the same [inventory reconciliation program](examples/inventory_reconciliation/)
with Relar and CPython, and comparing it with a Rust implementation:

| Workload | CPython 3.14.5 (ms) | Relar (ms) | Rust (ms) | Relar speedup vs. CPython |
| --- | ---: | ---: | ---: | ---: |
| Repeated requests (64 products in stock) | 77.66 | 26.69 | 23.34 | 2.91× |
| Distinct requests (16,384 products in stock) | 112.35 | 52.24 | 43.05 | 2.15× |

## Developer tools

The [memory-report comparator](examples/memory_report/README.md) compares report files and directories or runs two versions of ty over prepared projects. It can keep the child processes’ transcripts and atomically publish the Markdown report. The [workspace and atomic-output comparison](benchmarks/results/2026-10-21-native-workspaces.md) measures complete runs at 1.00–1.57× Rust’s time, file-only reports at 1.13–1.26×, and repeated report computation at 1.42–1.57× across ordinary and ThinLTO builds.

The [conformance reporter](examples/conformance_report/README.md) reads test groups from a typing checkout, runs both ty executables with timeouts, resolves diagnostic paths, and atomically publishes the Markdown or GitHub report. It compiles Ruff’s `Source`, `Classification`, and `Change` enums and the original `compute_stats` and `collect_diagnostic_entries` functions. Collecting 249 ordered diagnostics from 1,269 groups took 268 µs ordinarily and 215 µs with ThinLTO, or 1.45× and 1.51× the Rust reference’s time. [Collector and full-report benchmarks](benchmarks/results/2026-10-22-collector-ownership.md).

The [Python download catalog](examples/python_catalog/README.md) selects and renders 6,098 downloads from uv’s index of 58,721 release artifacts. It reads the shipped gzip index directly. Across plain and gzip inputs at 1× and 10×, median runs take 1.11–1.22× Rust’s time with peak RSS under 16 MiB. [Timings and allocation profiles](benchmarks/results/2026-10-22-catalog-ownership.md).

The [five-engine comparison](benchmarks/results/2026-10-21-native-tools.md) runs the catalog and conformance reporter against upstream and typed CPython, Codon, and Rust. Complete runs take 1.48–1.84× Rust’s time ordinarily and 1.33–1.77× with ThinLTO. Repeated computation also stays within 2× in both modes.

## Installation

Relar needs Clang 18 to compile and link programs. Release binaries target x86-64
and ARM64 GNU/Linux and are built on Ubuntu 24.04. Run these commands in an empty
directory to install a binary from [GitHub Releases](https://github.com/astral-sh/relar/releases).

```console
bundle="relar-$(uname -m)-unknown-linux-gnu"
gh release download --repo astral-sh/relar --pattern "$bundle.tar.gz" --pattern "$bundle.tar.gz.sha256"
sha256sum --check "$bundle.tar.gz.sha256"
tar -xzf "$bundle.tar.gz"
mkdir -p ~/.local/bin
cp "$bundle/relar" "$bundle/librelar_runtime.a" ~/.local/bin/
```

Keep both files together and add `~/.local/bin` to your `PATH`. Programs built
with Relar can run without the compiler, runtime archive, or Clang.

To build from Git, install the [Rust toolchain](rust-toolchain.toml) and Clang 18.

```console
git clone https://github.com/astral-sh/relar.git
cd relar
cargo build --workspace --release --locked
mkdir -p ~/.cargo/bin
cp target/release/relar target/release/librelar_runtime.a ~/.cargo/bin/
```

Make sure `~/.cargo/bin` is on your `PATH`.

## Getting started

Given `totals.py`:

```python
from sys import argv


def total(quantities: list[int]) -> int:
    result = 0
    for quantity in quantities:
        result += quantity
    return result


print(f"Total: {total([int(value) for value in argv[1:]])}")
```

Compile and run it in one command.

```console
relar run totals.py -- 10 20 12
# Output: Total: 42
```

Arguments after `--` go into the script’s `sys.argv`. Use `build` to keep the compiled executable.

```console
relar build totals.py
./totals 10 20 12
```

## Releasing

Run **Prepare release** on `main` to open the version and changelog PR. Approve its CI and merge it.
Then run **Release** on `main` to publish the two Linux archives and their checksums to GitHub Releases.
Select **dry-run** to build the archives without publishing.

## License

Relar is licensed under either of

- Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Vendored example code retains its original license; the Base45 codec uses
[BSD-2-Clause](examples/base45_codec/upstream/LICENSE), and the Luhn checksum
library uses its [upstream license](examples/luhn_checksum/upstream/LICENSE).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in Relar
by you, as defined in the Apache-2.0 license, shall be dually licensed as above, without any
additional terms or conditions.
