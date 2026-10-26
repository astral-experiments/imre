# google-crc32c

Imre builds a wheel from google-crc32c 1.8.0's upstream setuptools project.
Google's `setup.py` declares the C source, include directories, and `crc32c`
library used by its `value` and `extend` functions. [Upstream build configuration](https://github.com/googleapis/python-crc32c/blob/9652503fb296eef101d84b9b631f454cc7c1d538/setup.py) · [Native library](https://github.com/google/crc32c/tree/02e65f4fd3065d27b2e29324800ca6d04df16126)

The overlay supplies the native type declarations, a typed Python facade for
`Checksum`, and the BSD license for the bundled library. Setuptools supplies
upstream's distribution metadata and package data. [Wheel overlay](overlay/google_crc32c)

```python
from google_crc32c import Checksum, extend, value

assert value(b"123456789") == 0xe3069283
assert extend(value(b"12345"), b"6789") == value(b"123456789")
state = Checksum(b"12345")
state.update(b"6789")
assert state.hexdigest() == b"e3069283"
```

## Build and install

Run these commands from the Imre checkout with Clang, CMake, and Python
3.11 or later. Install the native dependency in a temporary prefix. Upstream
setuptools finds it through `CRC32C_INSTALL_PREFIX`.

```sh
work=$(mktemp -d)
git clone --depth=1 --branch 1.1.2 https://github.com/google/crc32c.git "$work/core"
git clone --depth=1 --branch v1.8.0 https://github.com/googleapis/python-crc32c.git "$work/python-crc32c"
export CC=clang CXX=clang++ CRC32C_INSTALL_PREFIX="$work/prefix" CRC32C_PURE_PYTHON=0
cmake -S "$work/core" -B "$work/core-build" -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$CRC32C_INSTALL_PREFIX" -DCMAKE_INSTALL_LIBDIR=lib \
  -DBUILD_SHARED_LIBS=OFF -DCMAKE_POSITION_INDEPENDENT_CODE=ON -DCRC32C_BUILD_TESTS=OFF \
  -DCRC32C_BUILD_BENCHMARKS=OFF -DCRC32C_USE_GLOG=OFF -DCMAKE_POLICY_VERSION_MINIMUM=3.5
cmake --build "$work/core-build" --target install
```

Build the wheel from that checkout using its original `pyproject.toml` and
`setup.py`. The Imre executable for this example uses the C++ standard
library supplied by the host toolchain.

```sh
python3 -m pip install build
cxx_lib=stdc++
if [ "$(uname -s)" = Darwin ]; then cxx_lib=c++; fi
imre wheel "$work/python-crc32c" --overlay examples/google_crc32c/overlay \
  --system-library "$cxx_lib" --out-dir "$work/dist"
python3 -m pip install --no-index --no-deps --no-compile --only-binary=:all: \
  --implementation im --python-version 3.14 --abi imre1 \
  --target "$work/site" "$work"/dist/*.whl
mkdir "$work/consumer"
cp examples/google_crc32c/driver.py "$work/consumer/main.py"
imre build "$work/consumer/main.py" --extension-path "$work/site" --clang "$CC" -o "$work/driver"
"$work/driver" public
```

The wheel retains upstream's `Requires-Python: >=3.9` metadata. Pip uses
Imre's Python 3.14 language target and `imre1` native ABI to select it.

## Validate and benchmark

```sh
python3 -m pip install pytest
LDFLAGS="-l$cxx_lib" python3 -m pip wheel --no-deps "$work/python-crc32c" --wheel-dir "$work/cpython-dist"
python3 -m pip install --no-index --no-deps --no-compile --target "$work/cpython" "$work"/cpython-dist/*.whl
"$CC" -O3 benchmarks/google_crc32c.c -I"$CRC32C_INSTALL_PREFIX/include" \
  "$CRC32C_INSTALL_PREFIX/lib/libcrc32c.a" -l"$cxx_lib" -o "$work/direct"
python3 tests/google_crc32c.py cpython --upstream "$work/python-crc32c" --cpython-package "$work/cpython"
python3 tests/google_crc32c.py imre --upstream "$work/python-crc32c" --imre-binary "$work/driver"
python3 tests/google_crc32c.py fallback --upstream "$work/python-crc32c"
python3 benchmarks/google_crc32c.py --direct "$work/direct" \
  --cpython-package "$work/cpython" --imre "$work/driver" --output "$work/timings.json"
```

Pytest and upstream's original Python wrappers run on CPython in all three
test modes. The Imre mode sends every native `value` and `extend` call to
the installed-wheel executable and checks its acknowledged call counts. This
also exercises upstream's lazy `Checksum.consume` method with its mock streams.
The native Imre executable separately checks the typed facade's methods.
The fallback mode imports the pure-Python implementation and selects the 26
cases that can run without a C extension. [Unmodified upstream tests](https://github.com/googleapis/python-crc32c/blob/9652503fb296eef101d84b9b631f454cc7c1d538/tests/test___init__.py)

The benchmark keeps each process alive, creates the payload before timing,
checks every result, and rotates the engine order between batches. On Linux
pass `--cpu N` to pin the parent and child processes to one available core.
The [published Linux timings](../../benchmarks/results/2026-10-23-google-crc32c.md)
used the [original build commands](https://github.com/astral-sh/imre/blob/d8e268d6885ce77d31bd3dd1c1c8fb223fd2a010/examples/google_crc32c/README.md).
