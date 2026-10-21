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
- Typed containers and arbitrary-precision integers
- Common control flow, comprehensions, and immediately consumed iterator pipelines
- Selected file and path operations

But not:

- Most of the Python standard library
- Dynamically typed storage (`Any` or unrestricted unions)
- Recursive calls or closures that capture variables
- Reflection or dynamic attributes
- Generator functions or async code
- Arbitrary decorators

## Installation

On 64-bit Linux, install the [Rust toolchain](rust-toolchain.toml) and Clang 18,
then build and install Relar from Git:

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
def total(quantities: list[int]) -> int:
    result = 0
    for quantity in quantities:
        result += quantity
    return result


print(f"Total: {total([10, 20, 12])}")
```

Compile and run the program:

```console
relar build totals.py
./totals
# Output: Total: 42
```

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
