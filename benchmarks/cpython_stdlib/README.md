# CPython standard-library compatibility benchmark

The `upstream` directory contains the complete, unmodified CPython 3.14.5 test modules and packages used by these benchmarks. The [manifest](UPSTREAM.json) records the release commit and each unmodified Git blob. [Upstream source](https://github.com/python/cpython/tree/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test) · [CPython license](upstream/LICENSE)

## Native string line boundaries and whitespace

With CPython 3.14.5's `Lib/test` on the import path, run
`tests/text_line_methods_upstream.py --compiler ... --runtime ...`. It runs the
unmodified `StrTest.test_splitlines` assertions on 11 native results and
`StrTest.test_isspace` on 12 native results plus six direct CPython literals.
The original Unicode invariant checks native results for all 1,112,064 Unicode
scalars; its 2,048 surrogate code points run on CPython. Both compiler modes
and optimization levels 0 and 3 are checked.

## Native regex substitution tests

The text suite also compiles three complete CPython `ReTests` methods:
`test_bug_449964`, `test_bug_449000`, and `test_bug_114660`. Their six native
assertions check capture references and replacement escapes in `re.sub`.
Run `tests/text.py` with the compiler, runtime, Python oracle and clang paths.

## Native CSV reader tests

`native_csv.py` compiles 18 complete, unchanged reader methods from CPython's
`TestDialectExcel`. A nineteenth multiline method supplies its original fixture
from CPython because it assigns a reserved builtin name. A typed file fixture compares every native row and field
against the original expected values. A second fixture compares errors, physical
line counts, Unicode, input mutation and incremental consumption with CPython.

## Native SHA-256 tests

`native_zipfile.py` runs four unchanged constructor error tests from CPython's ZIP suite. CPython owns the file setup and assertions; a native child opens the files and reports the exception. Separate successful opens check directory order, duplicate names and access after close. Pass the same arguments as the JSON runner, optionally with `--opt-level 0` or `3`.

`native_hashlib.py` takes the same arguments as the JSON runner. CPython’s eight SHA-1 and SHA-256 vector tests check the hex digest, raw digest and size from native hash objects. Alternate constructors and `file_digest` run on CPython. The original `test_large_update` method checks native SHA-1 updates and independent copies. Pass `--reachable-only` to test that compiler mode and `--wheelhouse` with the pinned Flask wheels to compare the unchanged Werkzeug `generate_etag` function with CPython.

The runner also compiles the unchanged `test_sha256_gil`, `test_sha256_update_over_4gb`, and `test_new_upper_to_lower` method bodies and their equality checks. CPython supplies the private extension threshold for the first fixture. The second updates the same object with a 1 MiB buffer 4,096 times, then adds a final 11 bytes and checks CPython’s expected digest.

## Native POSIX path cases

The seven selected upstream methods take a native Imre compiler and runtime.

```sh
python3 benchmarks/cpython_stdlib/native_paths.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a \
    --clang /path/to/clang --opt-level 3 --result native-paths.json
```

The runner verifies the upstream blob hashes and copies each complete method from CPython's `JoinTestBase` or `ReadTestBase` verbatim into a temporary source file. A typed fixture supplies the path constructor, working directory and `assertEqual`. The read fixture seeds `fileA` with the bytes used by CPython's local filesystem fixture. Both operands, path operations, equality checks and assertion counts execute in the compiled program. The concrete POSIX fixture uses Imre's native `pathlib.Path`. The selected methods check the name, stem, suffix, parent, slash operator, `joinpath` and `read_bytes`. Each result includes the upstream ID, the SHA-256 of the unmodified method and its assertion count. A rejected program or a failed native assertion gives the command a nonzero exit status.

Methods that require setup, decorators, process fixtures or a richer assertion helper need a corresponding adapter before the benchmark can credit them. Each added adapter should preserve the entire upstream method, including every operand and exception assertion. Keep platform skips, deliberate compatibility exclusions, unavailable test infrastructure and actual native failures separate in reports.

## Native POSIX shutil.which

`native_shutil.py` runs the vendored `TestWhich` and `TestWhichBytes` cases. Use the same command-line options as the JSON runner. CPython creates the original filesystem fixtures, changes the environment and working directory, and checks the results. Each `which` lookup runs in Imre with those process settings. Upstream platform skips retain their reasons. The two inherited `test_environ_path_missing` cases monkeypatch Python's `os.confstr` and `os.defpath` in-process. The report marks them as excluded and checks real missing-PATH behavior separately for strings and bytes.

## Native JSON calls with upstream unittest assertions

Four complete JSON error methods use a CPython 3.14.5 test interpreter for their assertions.

```sh
/path/to/cpython-3.14.5/python benchmarks/cpython_stdlib/native_json.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a \
    --clang /path/to/clang --result native-json.json
```

Use `--cpython-source /path/to/cpython-3.14.5` if the interpreter needs a source checkout for `test.support`. The original assertions run under unittest. Each input to `loads` is passed to a compiled Imre executable. The bridge forwards the native error category and its unmodified message. Methods which inspect structured error fields need additional native support before the adapter can credit them. Two successful array controls check the driver separately. The four methods run through both upstream backend fixture IDs, giving eight executions of four distinct methods.

Run `native_json_values.py` with the same arguments for the standard JSON value
and stream methods. The typed native driver receives primitive inputs as bytes
and returns loaded values using Python literal representations. The host runs
the verified assertions and reads the representation with `ast.literal_eval`.
Stream assertions receive the bytes that the native program wrote to its file.

## Native os calls with upstream unittest assertions

`native_os.py` runs CPython's `MiscTests.test_getcwd`, `test_getcwd_long_path`, and the PEP 519 string and bytes return tests. Each `getcwd` and `fspath` executes in the compiled driver. CPython owns the directory setup, cleanup, assertions and process transport. The runner also compares the error from a deleted working directory. Use the same compiler, runtime, clang and optional CPython source arguments as for `native_json.py`.

## Native POSIX os.path calls with upstream unittest assertions

`native_os_path.py` runs complete upstream POSIX tests for `join`, `basename`, `dirname`, `splitext`, `normpath` and `abspath`. CPython supplies unittest assertions and filesystem setup. Each tested operation calls a compiled Imre program in the fixture’s working directory. The result records each method’s native call count, including 100 calls from CPython’s 50 `normpath` cases for str and bytes. The unchanged `test_commonpath` accounts for 59 native calls, including the empty iterable, mixed types and absolute/relative conflicts. Pass the same compiler, runtime, clang, result and optional CPython source options as the JSON runner.

The two user-defined `__fspath__` tests are outside the typed path subset. On Linux the upstream non-ASCII `abspath` test requests a surrogateescaped string; the result records that exclusion. Separate comparisons check raw bytes in an undecodable directory, text paths that normalize away that directory, and relative and absolute paths after the working directory is deleted.

## Native logging

`native_logging.py` copies three complete `BuiltinLevelsTest` methods and `LoggerTest.test_root_logger_aliases` verbatim. Logger calls, message callbacks, and identity assertions run in each native executable. CPython's original `BaseTest.assert_log_lines` compares the native stderr with the expected rows emitted from the upstream method body. A separate fixture compares formatting, argument evaluation, configuration, and stream output byte for byte.

## Full CPython reference run

Use a test-enabled CPython interpreter built from the release in the manifest. Discover the suite names in the vendored directory, then run their matching copies in the release tree. This lets unittest discover the JSON and pathlib package tests with their original support modules.

```sh
reference_python=/path/to/cpython-3.14.5/python
"$reference_python" -m test --list-tests --testdir benchmarks/cpython_stdlib/upstream/Lib/test |
    "$reference_python" -m test -j1 --fromfile /dev/stdin
```

This run records what upstream passes and skips on the host. Native compatibility uses the separate Imre results. An infrastructure error in a stripped interpreter is visible until it is reproduced with a test-enabled CPython build.

`native_zip_members.py` drives the original stored and deflated bad-CRC test methods. The host makes fixtures and runs unittest assertions; the child opens its own `BytesIO`, reads members, checks CRCs, and returns the sequence of reads and errors. A separate control checks duplicate entries, metadata, independent readers and caller-owned stream lifetimes.

## Native Base64 cases

`native_base64.py` runs CPython's unchanged `BaseXYTestCase.test_b64encode`. Its exact-bytes URL-safe call runs in Imre; the other encoders, non-bytes buffers, and type errors run in CPython and are recorded separately. Additional native controls check output padding and strict ASCII decode errors against CPython.

## Native Counter cases

`native_counter.py` compiles seven complete CPython `TestCounter` methods into standalone executables: `test_total`, `test_invariant_for_the_in_operator` and the five comparison methods. The native assertion adapters count every executed assertion: one for the total, six for membership and two per comparison method. The manifest pins the entire upstream `test_collections.py` file. The typed subset uses integer counts, so the upstream `test_update`, which also stores `None`, is outside this gate.

```sh
python3 benchmarks/cpython_stdlib/native_counter.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a \
    --clang /path/to/clang --result native-counter.json
```
