# CPython 3.14.5 OS suites

| Suite | Discovered on Linux | Reference pass | Upstream skip | Scope |
| --- | ---: | ---: | ---: | --- |
| [test_os](reference-os-linux.json) | 375 | 317 | 58 | Primary OS suite, including inherited environment mapping tests |
| [test_posix](reference-posix-linux.json) | 179 | 146 | 33 | Related POSIX backend tests; track independently |
| [test_genericpath](reference-genericpath-linux.json) | 25 | 25 | 0 | Related path tests; inventory only |

The reference reports preserve each discovered ID and the upstream reason for each skip. All three suites are single modules at [the pinned commit](https://github.com/python/cpython/tree/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test). The OS manifest includes `mapping_tests.py`, from which `EnvironTests` inherits methods. The license and previously vendored sources link to the shared copies. None of the reference passes count as native coverage.

Run each manifest with a test-enabled release interpreter, as an unprivileged user. The [inventory runner](../../suite_inventory.py) compares Git blobs, invokes release discovery, runs regrtest in a child process and temporary directory, and rejects differences between discovered and reported IDs.

```sh
for suite in os posix genericpath; do
    "$CPYTHON" -B benchmarks/cpython_stdlib/suite_inventory.py \
        "benchmarks/cpython_stdlib/full/os/$suite.json" \
        --result "reference-$suite-linux.json" || exit 1
done
```

A user namespace mapping only the caller's UID/GID can cause `ChownFileTests.test_chown_gid` and `test_chown_without_permission` to get `EINVAL` for unmapped IDs. The recorded OS run used the normal devbox user's IDs; neither test was skipped.

## Descriptor execution

[Linux results](descriptors-linux.json) report 23 distinct methods at both `-O0` and `-O3`:

| Suite | Native body, assertions and OS operations; CPython fixtures | Host body and assertions; native OS operations |
| --- | ---: | ---: |
| `test_os` | 9 | 9 |
| `test_posix` | 3 | 2 |

The [shared runner](../../SUITE_RUNNER.md) runs each original module and instance fixture. In particular, `FileTests` removes its test file, `PosixTester` creates one and installs warning filters, and both modules have teardown hooks. The native child inherits this working directory and uses the same test filename. It registers descriptors for reverse-order cleanup and closes them in a `finally` block; the adapter accepts only `os.close` registrations. The invalid-descriptor fixture opens and closes a real descriptor inside Imre. Every row names the CPython hooks; none is counted as an entirely native fixture.

For the eleven host-driven methods, compiled Imre code shares the host process's descriptor table with upstream `fcntl` and file objects. The host supplies an independent check; the operations under test execute in Imre. The older `native_os.py` getcwd/fspath results are separate.

Run `native_descriptors.py` and `host_descriptors.py` here with the pinned reference interpreter. Both accept `--imre`, `--runtime`, `--clang`, `--opt-level 0|3`, `--result`, and an optional `--cpython-source`. Each run uses temporary fixtures. Host-driven calls expose only explicitly routed operations. An absent bridge operation fails the test.

The two [Linux pipe2 methods](PIPE2.md) are separate from the 23 descriptor methods above.

## Structured OS errors

For a failed descriptor operation, the compiled entry returns the caught OS exception subclass, errno and strerror. The host checks the class before it raises the transported error for the upstream assertions. File errors retain the original str or bytes object; the runtime ownership test covers a second filename and a bytes path containing 0xff.

The four public fields use the same stored errno and strerror arguments as exception repr. The error-fields probe compares both display forms and all fields with the pinned interpreter for missing str/bytes filenames, a bad descriptor, and a message-only error. It contributes no upstream test ID.

## Environment and path values

[Five unique OS IDs](values-linux.json) were run at `-O0` and `-O3` with [the shared fixture runner](../../SUITE_RUNNER.md). The complete unchanged bodies for `TestPEP519.test_return_bytes`, `test_return_string` and `EnvironTests.test_items` run native OS operations and assertions under CPython's hooks. The latter inherits the real environment set up by upstream. `MiscTests.test_getcwd` is host-driven; its [shared assertion prerequisite](https://github.com/astral-sh/imre/pull/508#issuecomment-6392091565) is tracked separately. The long-path method uses native `getcwd` and host `mkdir`, `chdir` and path joining, so its result is mixed.

Run `value_cases.py` with the release interpreter and `--imre`, `--runtime`, `--clang`, `--opt-level 0|3`, `--result`, and optionally `--cpython-source`. This evidence supersedes the execution modes for the four IDs in the older `native_os.py` gate. They count once.

## Versioned occurrence reports

Each Linux reference has a matching pair of [v1 reports](../../NATIVE_EVIDENCE.md).
Each occurrence appears once per optimization level under its original class.
The repeated optimization levels and the companion suites are not additional
primary OS tests.

| Suite | Occurrences per level | Source and outcomes | O0 / O3 execution evidence |
| --- | ---: | --- | --- |
| test_os | 375 | [reference](reference-os-linux.json) | [evidence](evidence-os-linux.json) |
| test_posix | 179 | [reference](reference-posix-linux.json) | [evidence](evidence-posix-linux.json) |
| test_genericpath | 25 | [reference](reference-genericpath-linux.json) | [evidence](evidence-genericpath-linux.json) |

Run `python3 benchmarks/cpython_stdlib/full/os/coverage.py` to validate all
three pairs without building Imre. Pass `--result FILE` to export all rows.
Use release CPython to regenerate with `evidence.py --cpython-source SOURCE`.
The generator repeats exact discovery in a temporary directory, verifies the
release blobs and complete native method hashes, and checks both optimization
levels. The 91 reference skips retain their original conditions, fixtures,
source lines and reasons. Five private-only _fspath methods remain reference-only;
no reviewed exclusion is claimed. The older artifacts retain method hashes,
native calls and host setup details for auditing. A supplemental configuration
does not replace a canonical skip.

[Method preflight](method-preflight-linux.json) records a separate native build
at each level for the initial 295 OS, 143 POSIX and 25 genericpath reference-passing
occurrences outstanding after #506. Later successful executions take precedence over
the earlier diagnostics in the versioned reports. Each build takes the complete verified definition, its
actual imported names, and the shared native TestCase fixture. Eight generated
invalid-descriptor methods receive the operation name from their original closure.
The first preflight found two compilable outer methods: the unchanged subprocess requests for
`os.setregid(-1, -1)` and `os.setreuid(-1, -1)`. Both now run the original child source in Imre;
the [credential results](CREDENTIALS.md) distinguish these executions from the earlier CPython children.
For the remaining methods, the first compiler diagnostic, source line, method hash, Imre revision and
compiler binary hash are recorded. No process or filesystem operation in an
unreviewed test is executed during these builds.

For example, `EnvironTests.test_ior_operator_invalid_dicts` reaches
`with self.assertRaises(TypeError)` at test_os.py:1377; the [unittest context
work](https://github.com/astral-sh/imre/pull/587#issuecomment-6392803799) covers
that fixture requirement. Other first errors belong to upstream case helpers,
such as `WalkTests.walk`, or to a product import such as `posix`. A compiler
diagnostic describes the first failed build step, not the result of running the
method. The [canonical rollup](../native_report.py) counts 27 native bodies with
host fixtures, 15 host-driven native operations, two mixed bodies, 439 blockers,
five private reference-only cases, and 91 upstream skips at each level.

To refresh these diagnostics, run `method_preflight.py` twice with the same
`--imre`, `--runtime`, `--clang` and `--cpython-source`, once with each
`--opt-level 0|3` and a distinct `--result`. Pass both files to
`evidence.py --method-results O0.json O3.json --compiler-commit FULL_SHA`.
CI independently reruns the 44 exercised methods at both levels and
compares their results with the denominator reports.
