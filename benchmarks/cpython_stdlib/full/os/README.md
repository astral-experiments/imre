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
