# CPython standard-library compatibility benchmark

The `upstream` directory contains the complete, unmodified CPython 3.14.5 test modules and packages used by these benchmarks. The [manifest](UPSTREAM.json) records the release commit and each unmodified Git blob. [Upstream source](https://github.com/python/cpython/tree/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test) · [CPython license](upstream/LICENSE)

## Native SHA-256 vector calls

`native_hashlib.py` takes the same arguments as the JSON runner. The four original `HashLibTestCase.test_case_sha256_*` methods and their shared helper run in full. Each method sends the public constructor's one-shot hexadecimal digest to native Relar. Raw digests, digest sizes, alternate constructors and file hashing execute on CPython. The JSON report counts the four native vector calls as mixed-method coverage and records the host calls separately.

## Native POSIX path cases

The seven selected upstream methods take a native Relar compiler and runtime.

```sh
python3 benchmarks/cpython_stdlib/native_paths.py \
    --relar /path/to/relar --runtime /path/to/librelar_runtime.a \
    --clang /path/to/clang --opt-level 3 --result native-paths.json
```

The runner verifies the upstream blob hashes and copies each complete method from CPython's `JoinTestBase` or `ReadTestBase` verbatim into a temporary source file. A typed fixture supplies the path constructor, working directory and `assertEqual`. The read fixture seeds `fileA` with the bytes used by CPython's local filesystem fixture. Both operands, path operations, equality checks and assertion counts execute in the compiled program. The concrete POSIX fixture uses Relar's native `pathlib.Path`. The selected methods check the name, stem, suffix, parent, slash operator, `joinpath` and `read_bytes`. Each result includes the upstream ID, the SHA-256 of the unmodified method and its assertion count. A rejected program or a failed native assertion gives the command a nonzero exit status.

Methods that require setup, decorators, process fixtures or a richer assertion helper need a corresponding adapter before the benchmark can credit them. Each added adapter should preserve the entire upstream method, including every operand and exception assertion. Keep platform skips, deliberate compatibility exclusions, unavailable test infrastructure and actual native failures separate in reports.

## Native POSIX shutil.which

`native_shutil.py` runs the vendored `TestWhich` and `TestWhichBytes` cases. Use the same command-line options as the JSON runner. CPython creates the original filesystem fixtures, changes the environment and working directory, and checks the results. Each `which` lookup runs in Relar with those process settings. Upstream platform skips retain their reasons. The two inherited `test_environ_path_missing` cases monkeypatch Python's `os.confstr` and `os.defpath` in-process. The report marks them as excluded and checks real missing-PATH behavior separately for strings and bytes.

## Native JSON calls with upstream unittest assertions

Four complete JSON error methods use a CPython 3.14.5 test interpreter for their assertions.

```sh
/path/to/cpython-3.14.5/python benchmarks/cpython_stdlib/native_json.py \
    --relar /path/to/relar --runtime /path/to/librelar_runtime.a \
    --clang /path/to/clang --result native-json.json
```

Use `--cpython-source /path/to/cpython-3.14.5` if the interpreter needs a source checkout for `test.support`. The original assertions run under unittest. Each input to `loads` is passed to a compiled Relar executable. The bridge forwards the native error category and its unmodified message. Methods which inspect structured error fields need additional native support before the adapter can credit them. Two successful array controls check the driver separately. The four methods run through both upstream backend fixture IDs, giving eight executions of four distinct methods.

Run `native_json_values.py` with the same arguments for the standard JSON value
and stream methods. The typed native driver receives primitive inputs as bytes
and returns loaded values using Python literal representations. The host runs
the verified assertions and reads the representation with `ast.literal_eval`.
Stream assertions receive the bytes that the native program wrote to its file.

## Native os calls with upstream unittest assertions

`native_os.py` runs CPython's `MiscTests.test_getcwd`, `test_getcwd_long_path`, and the PEP 519 string and bytes return tests. Each `getcwd` and `fspath` executes in the compiled driver. CPython owns the directory setup, cleanup, assertions and process transport. The runner also compares the error from a deleted working directory. Use the same compiler, runtime, clang and optional CPython source arguments as for `native_json.py`.

## Native POSIX os.path calls with upstream unittest assertions

`native_os_path.py` runs CPython's complete POSIX `test_join`, `test_basename`, `test_dirname` and `test_splitext` methods. Pass the same compiler, runtime, clang, result and optional CPython source options as the JSON runner. The original unittest setup and comparisons run in CPython. Each tested path operation, including those made by the `splitextTest` helper, calls a compiled Relar program. The runner checks the native call count for every test.

## Full CPython reference run

Use a test-enabled CPython interpreter built from the release in the manifest. Discover the suite names in the vendored directory, then run their matching copies in the release tree. This lets unittest discover the JSON and pathlib package tests with their original support modules.

```sh
reference_python=/path/to/cpython-3.14.5/python
"$reference_python" -m test --list-tests --testdir benchmarks/cpython_stdlib/upstream/Lib/test |
    "$reference_python" -m test -j1 --fromfile /dev/stdin
```

This run records what upstream passes and skips on the host. Native compatibility uses the separate Relar results. An infrastructure error in a stripped interpreter is visible until it is reproduced with a test-enabled CPython build.
