# Imre

An ahead-of-time compiler for typed Python, written in Rust.

```python
# hello.py
def greet(name: str) -> str:
    return f"Hello, {name}!"


print(greet("world"))
```

On Apple Silicon macOS or 64-bit Linux, build and run the executable.

```console
$ imre build hello.py
$ ./hello
Hello, world!
```

> [!WARNING]
> This README includes human edits; all code changes, PR summaries, and additional
> documentation were authored entirely by GPT-6 Astra in [Codex](https://openai.com/codex/).
> Use at your own risk.

## Language support

Imre compiles a restricted subset of Python into native executables that run
without a Python installation.

For a complete example, Imre can build Ruff’s conformance reporter directly
from its original Python file. The executable runs two versions of ty against
a typing checkout and renders their differences. [Run the reporter](examples/conformance_report/README.md#run-directly-from-a-checkout).

Imre also runs Ruff’s original memory reporter. It compares saved JSON captures
or runs two versions of ty on flake8, Sphinx, Prefect, and Trio.
[Run the memory reporter](examples/memory_report/README.md#run-ruffs-original-script).

Imre builds Ruff’s ecosystem checker from its original Python package. It clones
58 repositories and compares the diagnostics from two Ruff binaries.
[Run the ecosystem checker](examples/ruff_ecosystem/README.md#run-the-original-check-command).

Imre supports:

- Statically resolved imports, including [rebuilt native extension wheels](docs/extensions.md)
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

Types are part of Imre's compilation contract: variables, containers, and class
fields have known types, and each function has one declared or inferred
signature. This lets Imre choose concrete representations. It aims to preserve
Python's behavior for supported operations, including arbitrary-precision integers.

Imre's runtime uses reference counting and restricts object graphs to avoid
cycles. Compiled programs run without a Python installation. Their imports must
use supported Python or native extensions rebuilt against Imre’s `Python.h`.

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

Relar and CPython 3.14.5 run the same Python programs. The Rust references read the same inputs and produce the same outputs.

![Median elapsed time and interquartile range for Rust, CPython and Relar on six workloads. Exact medians follow in the table.](benchmarks/results/runtime-suite.svg)

Each point is the median of 12 fresh processes on one AMD EPYC Milan CPU, including startup, imports and file I/O. Lower is faster.

| Workload | Input per process | Rust (ms) | CPython (ms) | Relar (ms) | CPython / Relar |
| --- | --- | ---: | ---: | ---: | ---: |
| Inventory (repeated) | 16,384 requests, 64 SKUs | 25.30 | 77.11 | 30.04 | 2.57× |
| Inventory (distinct) | 16,384 requests, 16,384 SKUs | 45.69 | 112.32 | 53.05 | 2.12× |
| Base45 decode | 64 × 16 KiB decoded | 9.24 | 183.81 | 15.54 | 11.83× |
| Luhn checksum | 65,536 × 16 digits | 5.28 | 149.53 | 13.08 | 11.43× |
| Telemetry report | 81 × 4,096 aligned pairs | 9.34 | 120.19 | 13.31 | 9.03× |
| Conformance report | 146 files, two captured ty runs | 23.59 | 247.01 | 33.04 | 7.48× |

[Workloads, timing ranges and reproduction](benchmarks/runtime-suite.md) · [Raw samples](benchmarks/results/runtime-suite.json)

## Installation

Install Imre on Linux or Apple Silicon macOS with the standalone installer.

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://labs.astral.sh/imre/install.sh | sh
```

The x86-64 and ARM64 Linux releases include Clang 18, LLD 18, and the native
link inputs that `imre build` and `imre run` need. Keep the `imre-toolchain`
directory beside `imre` when moving an installation. The Linux binaries require
glibc 2.39 or later. On macOS, install the Xcode Command Line Tools
(`xcode-select --install`). For a source build on Debian or Ubuntu, install
`clang-18`. Installing `lld-18` also enables faster links on x86-64.
To install a specific version, use `https://labs.astral.sh/imre/releases/<version>/install.sh`.

To build from source, use the [Rust toolchain](rust-toolchain.toml).

```console
git clone https://github.com/astral-sh/imre.git
cd imre
cargo build --workspace --release --locked
./target/release/imre --version
```

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
imre run totals.py -- 10 20 12
# Output: Total: 42
```

Arguments after `--` go into the script’s `sys.argv`. Use `build` to keep the compiled executable.

Use `imre check --watch totals.py` to see diagnostics as you edit the script or its imports. Press Ctrl-C to stop.

`build` and `run` share cached executables and module objects by default with
the bundled Linux toolchain. Source builds on x86-64 Debian and Ubuntu also
support this cache with the distribution's Clang 18 toolchain. The build summary
goes to stderr. Pass `-v` to see each module decision or `-q` to suppress
progress. [Build cache and code generation](docs/subset.md#incremental-builds)

```console
imre build totals.py
./totals 10 20 12
```

`build` and `run` use the system allocator by default. On Linux, pass
`--allocator jemalloc` to use jemalloc for the compiled program's heap.
For ThinLTO, select the allocator with `--runtime`. [ThinLTO build instructions](docs/subset.md#build-and-validate)

Imre removes unused code when linking executables. Pass `--no-gc-sections` to keep it,
or `--verbose-link` to see the Clang command. [Link options and environment variables](docs/subset.md#link-options)

## Releasing

Run **Prepare release** on `main` to open the version and changelog PR. Approve its CI and merge it.
Then run **Release** on `main` to publish the Linux and macOS archives and their checksums to GitHub Releases.
Select **dry-run** to build the archives without publishing.

## License

Imre is licensed under either of

- Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Vendored example code retains its original license; the Base45 codec uses
[BSD-2-Clause](examples/base45_codec/upstream/LICENSE), and the Luhn checksum
library uses its [upstream license](examples/luhn_checksum/upstream/LICENSE).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in Imre
by you, as defined in the Apache-2.0 license, shall be dually licensed as above, without any
additional terms or conditions.
