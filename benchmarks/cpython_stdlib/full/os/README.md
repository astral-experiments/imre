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

[Linux results](descriptors-linux.json) list the results by upstream ID. Nine `test_os` bodies and one `test_posix` body run as complete native methods at both `-O0` and `-O3`. The compiled fixture checks the original assertions, registers descriptors for reverse-order cleanup, and closes them in a `finally` block. Only `os.close` registrations are accepted. For three additional OS methods and two POSIX methods, CPython drives a compiled Imre entry point in the same process, so upstream `fcntl` and host file objects can observe the actual OS descriptors. These IDs are disjoint. The older `native_os.py` getcwd/fspath results are separate.

Run `native_descriptors.py` and `host_descriptors.py` here with the pinned reference interpreter. Both accept `--imre`, `--runtime`, `--clang`, `--opt-level 0|3`, `--result`, and an optional `--cpython-source`. Each run uses temporary fixtures. Host-driven calls expose only explicitly routed operations. An absent bridge operation fails the test.
