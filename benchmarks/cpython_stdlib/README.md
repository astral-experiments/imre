# CPython standard-library compatibility benchmark

The `upstream` directory contains the complete CPython 3.14.5 test entries for `sys`, `os`, `genericpath`, `posixpath`, `pathlib`, `json` and `subprocess`, including files in the two test packages. The [manifest](UPSTREAM.json) records the release commit and each unmodified Git blob. [Upstream source](https://github.com/python/cpython/tree/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test) · [CPython license](upstream/LICENSE)

## Native POSIX path cases

The seven selected upstream methods take a native Relar compiler and runtime.

```sh
python3 benchmarks/cpython_stdlib/native_paths.py \
    --relar /path/to/relar --runtime /path/to/librelar_runtime.a \
    --clang /path/to/clang --opt-level 3 --result native-paths.json
```

The runner verifies the upstream blob hashes and copies each complete method from CPython's `JoinTestBase` or `ReadTestBase` verbatim into a temporary source file. A typed fixture supplies the path constructor, working directory and `assertEqual`. The read fixture seeds `fileA` with the bytes used by CPython's local filesystem fixture. Both operands, path operations, equality checks and assertion counts execute in the compiled program. The concrete POSIX fixture uses Relar's native `pathlib.Path`. The selected methods check the name, stem, suffix, parent, slash operator, `joinpath` and `read_bytes`. Each result includes the upstream ID, the SHA-256 of the unmodified method and its assertion count. A rejected program or a failed native assertion gives the command a nonzero exit status.

Methods that require setup, decorators, process fixtures or a richer assertion helper need a corresponding adapter before the benchmark can credit them. Each added adapter should preserve the entire upstream method, including every operand and exception assertion. Keep platform skips, deliberate compatibility exclusions, unavailable test infrastructure and actual native failures separate in reports.

## Native JSON calls with upstream unittest assertions

Four complete JSON error methods use a CPython 3.14.5 test interpreter for their assertions.

```sh
/path/to/cpython-3.14.5/python benchmarks/cpython_stdlib/native_json.py \
    --relar /path/to/relar --runtime /path/to/librelar_runtime.a \
    --clang /path/to/clang --result native-json.json
```

Use `--cpython-source /path/to/cpython-3.14.5` if the interpreter needs a source checkout for `test.support`. The original assertions run under unittest. Each input to `loads` is passed to a compiled Relar executable. The bridge forwards the native error category and its unmodified message. Methods which inspect structured error fields need additional native support before the adapter can credit them. Two successful array controls check the driver separately. The four methods run through both upstream backend fixture IDs, giving eight executions of four distinct methods.

## Full CPython reference run

Use a CPython interpreter built from the exact release in the manifest with its normal test modules installed. Its `Lib/test` provides the shared `test.support`, helper programs and reference runner for the seven suites.

```sh
/path/to/cpython-3.14.5/python -m test -j1 \
    test_sys test_os test_genericpath test_posixpath test_pathlib test_json test_subprocess
```

This run records what upstream passes and skips on the host. Native compatibility uses the separate Relar results. An infrastructure error in a stripped interpreter is visible until it is reproduced with a test-enabled CPython build.
