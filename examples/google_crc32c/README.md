# google-crc32c

This wheel rebuilds the original google-crc32c 1.8.0 C extension against
Imre's `Python.h`. Its `value` and `extend` functions call Google's CRC32C
1.1.2 library. Both native builds and the direct C benchmark use the same
position-independent release archive. [Python source](https://github.com/googleapis/python-crc32c/blob/9652503fb296eef101d84b9b631f454cc7c1d538/src/google_crc32c/_crc32c.c) · [Native library](https://github.com/google/crc32c/tree/02e65f4fd3065d27b2e29324800ca6d04df16126)

The wheel includes a typed Python facade. It exports the two native functions
and a checksum object supporting initialization, update, copy, digest, and hex
digest. The compile-time signatures accept `bytes` and `int` and preserve the
original methods' positional-only calling convention.

```python
from google_crc32c import Checksum, extend, value

assert value(b"123456789") == 0xe3069283
assert extend(value(b"12345"), b"6789") == value(b"123456789")
state = Checksum(b"12345")
state.update(b"6789")
assert state.hexdigest() == b"e3069283"
```

## Build and install

The following commands run from the Imre checkout on x86-64 Linux with
Clang, CMake, a Make or Ninja generator, and a CPython environment containing
pip, wheel, packaging, and pytest. Set `work` to a fresh directory outside the
checkout. The published Linux timings used `CMAKE_GENERATOR=Ninja`.

```sh
work=$(mktemp -d)
git clone --depth=1 --branch 1.1.2 https://github.com/google/crc32c.git "$work/core"
python3 benchmarks/google_crc32c_build.py --crc32c-source "$work/core" \
  --output "$work" --imre-include crates/imre-runtime/include --cc clang-18 --cxx clang++-18
python3 scripts/pack_extension_wheel.py "$work/wheel" --name google-crc32c \
  --version 1.8.0 --output "$work/dist"
python3 -m pip install --no-index --no-deps --no-compile --only-binary=:all: \
  --implementation im --python-version 1 --abi imre1 \
  --target "$work/site" "$work"/dist/*.whl
mkdir "$work/consumer"
cp examples/google_crc32c/driver.py "$work/consumer/main.py"
imre build "$work/consumer/main.py" --extension-path "$work/site" -o "$work/driver"
"$work/driver" public
```

The builder verifies the vendored C, Python, and test files and the CMake and
native source inputs against [their pinned hashes](UPSTREAM.json). The wheel
includes both upstream licenses and records its C++ runtime link dependency.

## Validate and benchmark

```sh
python3 tests/google_crc32c.py cpython --cpython-package "$work/cpython"
python3 tests/google_crc32c.py imre --imre-binary "$work/driver"
python3 tests/google_crc32c.py fallback
python3 benchmarks/google_crc32c.py --direct "$work/direct" \
  --cpython-package "$work/cpython" --imre "$work/driver" --output "$work/timings.json"
```

Pytest and upstream's original Python wrappers run on CPython in all three
test modes. The Imre mode sends every native `value` and `extend` call to
the installed-wheel executable and checks its acknowledged call counts. This
also exercises upstream's lazy `Checksum.consume` method with its mock streams.
The native Imre executable separately checks the typed facade's methods.
The fallback mode imports the pure-Python implementation and selects the 26
cases that can run without a C extension. [Unmodified upstream tests](upstream/test___init__.py)

The benchmark keeps each process alive, creates the payload before timing,
checks every result, and rotates the engine order between batches. On Linux
pass `--cpu N` to pin the parent and child processes to one available core.
