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

Reproduce the walking-head baseline with the test-enabled 3.14.5 interpreter:

```sh
python -B benchmarks/cpython_stdlib/full/pathlib/evidence.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --imre-commit 906b10a80a61bbf4efc5cd05cc5c2ae82689e32a \
    --body-report benchmarks/cpython_stdlib/full/pathlib/body-probes-linux.json \
    --cohort lexical --cohort files --cohort traversal --cohort components --cohort walk --cohort basics \
    --result /tmp/pathlib-baseline.json
```

Run the metadata cohort with a compiler that implements Path.info, then verify
those six observations against the versioned per-occurrence records:

```sh
python -B benchmarks/cpython_stdlib/full/pathlib/evidence.py \
    --imre /path/to/metadata/imre --runtime /path/to/metadata/libimre_runtime.a --clang /path/to/clang \
    --cohort metadata --result /tmp/pathlib-metadata.json
python3 benchmarks/cpython_stdlib/full/pathlib/metadata_receipt.py /tmp/pathlib-metadata.json
python3 benchmarks/cpython_stdlib/suite_evidence.py \
    benchmarks/cpython_stdlib/full/pathlib/reference-linux.json \
    benchmarks/cpython_stdlib/full/pathlib/evidence-linux.json
```

The baseline adapters and complete-method probes used the compiler and runtime
from walking head `906b10a`. The six newer Path.info records use the measured
revision `13b6945`; all other rows retain their original revision and roles.
The 978 baseline probe results and diagnostics matched between optimization levels.

For both Path and PosixPath, the original existence, directory and file cache
methods check a missing name, create a file or directory and query the same view
again. The complete bodies, filesystem mutations and 24 boolean assertions per
level run in Imre; CPython supplies the original fixture setup and teardown.
The [filesystem comparison](../../../../tests/fixtures/paths/metadata_views.py)
compares 31 output lines, including stale views, fresh Path queries and directory
entry information for a dangling symlink whose target appears and disappears.
The last four lines check a scanned regular file replaced with a dangling symlink.

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

The [component runner](native_components.py) measures 36 further occurrences on
five concrete or pure path classes, including `PureWindowsPath` on Linux. The
three `test_suffixes` bodies and their list comparisons run natively. The other
33 run the original body and assertions on the host with each path held by Imre.
Across those methods 435 `relative_to`, 168 `is_relative_to` and 177 name, stem
or suffix calls run natively per level, including the upstream invalid inputs.

The [walk runner](native_walk.py) adds seven occurrences. The unchanged bottom-up
and two symlink methods run natively with their upstream host fixtures. CPython
drives the native cursors in the depth-limit and 100-open-cursor methods. The
top-down and pruning methods also mutate or sort yielded name lists in CPython;
the driver applies directory edits to the retained native list before resuming.
These two are mixed. The callback-after-rename method still needs a native error callback and rename.
The separate [walking matrix](walk_matrix.py) compares 216 combinations at each
level, including edits that insert duplicate, absolute, empty, and dotted names.
It checks each yielded path's spelling, logical parent, basename and equality,
along with the order and contents of both yielded lists.

The [whole-method sweep](body_sweep.py) independently attempted all 978 reference
passes under their original fixture lifecycles. It profiles assertion operand **types**
in CPython, then compiles the original method and its `_check` helpers using native
receivers. It tries native unittest.TestCase assertions, then a typed native assertion
fixture if compilation fails. It retains each diagnostic and never reruns a completed
method with another fixture. It verifies the pinned definitions and preserves original wrappers. At
each level, 111 entries completed in Imre (83 with native unittest and 28 with
typed native assertions), 407 encountered their first compiler
diagnostic in the upstream method, 421 in a fixture or dependency, and 39 had
an original wrapper that the native fixture could not execute. A completed probe
earns pass credit only after its operations and assertions have been verified.

The [counted adapter](native_basics.py) verifies 14 additional completions:
empty paths and string roundtrips for five classes (including `PureWindowsPath`),
POSIX absolute roots and existing-root `mkdir` calls. The 14 occurrences executed
56 native constructors and 52 native assertions per level. The two `mkdir` methods
require successful calls and contain no explicit assertions.

Regenerate the [per-ID probe report](body-probes-linux.json) with the compiler
and runtime built from the specified Imre commit. `--work` retains generated
sources and results. Linked probe binaries are released after each run.

```sh
python -B -X warn_default_encoding benchmarks/cpython_stdlib/full/pathlib/body_sweep.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --work /tmp/pathlib-bodies
python3 benchmarks/cpython_stdlib/full/pathlib/body_report.py --work /tmp/pathlib-bodies \
    --imre-commit 906b10a80a61bbf4efc5cd05cc5c2ae82689e32a \
    --result benchmarks/cpython_stdlib/full/pathlib/body-probes-linux.json
```

| Execution at each of O0 and O3 | Occurrences |
| --- | ---: |
| Native body and operation, native assertions if present, host fixture | 122 |
| Host body and assertions, native operation | 54 |
| Mixed execution | 11 |
| Blocked: diagnostic in the original body | 354 |
| Blocked: fixture or dependency diagnostic | 398 |
| Blocked: original fixture wrapper | 39 |
| Upstream reference skip | 391 |
| **Total** | **1,369** |

There are no fully native fixture lifecycles, extra exclusions, or native execution
failures in this run. Some methods that the general probe could not compile have
a more specific native or host-driven adapter. Those occurrences keep the observed
execution roles; the 791 blocked rows above have no successful adapter. The
per-ID records give the original source location, diagnostic or wrapper, and any
missing fixture behavior. A blocked row is an outstanding failure to run the method.

[Whole-package startup](whole-suite-startup.json) is tracked separately. On
`76d74f9` normal imports stopped at the leading name guard in `test.support`.
On the combined compiler head `03977539` both check modes reached the
`annotationlib` import. The wrong-name guard raised the reference `ImportError`
without running child initialization. Published main `c306c73` confirmed that stop for
both entry points. After annotationlib landed in #586, all four checks on walking
head `ea9845a` reached `inspect` in `test/support/__init__.py:9`. The prospective
walk merge against main `13759fb` confirmed the same four outcomes. Once #784
landed, both entry points in both check modes reached `_opcode` at
`test/support/__init__.py:11`. The published [walk merge](https://github.com/astral-sh/imre/pull/785)
confirmed that result on main `4fe38f6`. None
of these startup checks executed an upstream method.
