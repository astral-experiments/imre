# CPython 3.14.5 pathlib

The [pinned manifest](../../UPSTREAM.json) covers the [PSF license](../../upstream/LICENSE)
and all 12 files in [`Lib/test/test_pathlib`](../../upstream/Lib/test/test_pathlib),
including its package discovery hook and lexical, local and ZIP fixtures. The
reference is CPython commit `5607950ef232dad16d75c0cf53101d9649d89115`.

| Child suite | Discovered | CPython pass | CPython skip |
| --- | ---: | ---: | ---: |
| test_copy | 52 | 48 | 4 |
| test_join | 57 | 57 | 0 |
| test_join_posix | 6 | 6 | 0 |
| test_join_windows | 28 | 28 | 0 |
| test_pathlib | 1,139 | 752 | 387 |
| test_read | 57 | 57 | 0 |
| test_write | 30 | 30 | 0 |
| **Total** | **1,369** | **978** | **391** |

Reproduce the [per-ID Linux reference](reference-linux.json) using a test-enabled
CPython built at the pin:

```sh
python -B benchmarks/cpython_stdlib/full/pathlib/reference.py --output /tmp/pathlib-reference.json
```

A reference row is either `"pass"` or an upstream outcome with its original
reason. The script compares the oracle package
byte for byte with the vendored sources, independently loads all IDs using the
package hook, and checks that regrtest executed exactly those IDs. It enables
default-encoding warnings and uses a short temporary directory for Unix
sockets. All 28 Windows join tests and 52 of the 60 `PureWindowsPathTest`
methods run on Linux. The concrete `WindowsPathTest` class skips on this host.

## Native evidence

[`evidence-linux.json`](evidence-linux.json) contains one [v1 record](../../NATIVE_EVIDENCE.md)
per occurrence at each optimization level. The package discovery hook already
includes all seven child suites; adding their counts again would duplicate them.
All 391 skips name their fixture, original reason and pinned source condition.
There are no additional exclusions.

Generate both levels with the test-enabled 3.14.5 interpreter and validate them:

```sh
python -B benchmarks/cpython_stdlib/full/pathlib/evidence.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --result benchmarks/cpython_stdlib/full/pathlib/evidence-linux.json
python3 benchmarks/cpython_stdlib/suite_evidence.py \
    benchmarks/cpython_stdlib/full/pathlib/reference-linux.json \
    benchmarks/cpython_stdlib/full/pathlib/evidence-linux.json
```

The lexical runner dispatches each original method under its upstream class.
It checks that class and its separator before starting the typed native receiver.
The file reader receives the path made by the original instance setup. File
writes run the original method and its assertions in CPython and forward each
write, including invalid input, to Imre. The reports preserve those different
roles. [Lexical assertion counts](lexical-results.json), [file operation counts](file-results.json)
and the [earlier typed path gate](../../native_paths.py) provide additional detail.

The [traversal runner](native_traversal.py) adds 31 unchanged occurrences. Six
compile their entire bodies, including the two fixture variants that hold 100
glob/rglob iterators open and check 3,100 yielded paths each. Sixteen have host
bodies and assertions that drive native operations. The other nine also use a
host parser, third-party path conversion, or an iterator type assertion on a
host proxy; their records identify the specific use. The [glob comparison](glob_matrix.py)
checks the order, duplicates and errors of 1,920 requests at each level independently
of the unittest counts. Six additional requests exercise a directory symlink cycle.
Repeated scans of the same cycle by CPython can stop at different depths on this
Linux host. The original tests compare paths through the first link and discard
deeper repetitions; the report retains the actual number of native iterator pulls
on each run.

| Execution at each of O0 and O3 | Occurrences |
| --- | ---: |
| Native body, assertions and operation with host fixture | 96 |
| Host body and assertions, native operation | 19 |
| Mixed execution | 9 |
| Reference only | 854 |
| Upstream reference skip | 391 |
| **Total** | **1,369** |

This revision records zero fully native fixture lifecycles, extra
exclusions, native failures or method blockers. The package startup blocker
below is independent of those method outcomes. Three host-driven occurrences
also record why their whole body does not yet compile: list `+=` in `test_iterdir`
and the unannotated `_check` local function in both `test_glob_case_sensitive` fixtures.

[Whole-package startup](whole-suite-startup.json) is tracked separately. On
`76d74f9` normal imports stopped at the leading name guard in `test.support`.
On the combined compiler head `03977539` both check modes reached the
`annotationlib` import. The wrong-name guard raised the reference `ImportError`
without running child initialization. None of these checks executes an upstream method.
