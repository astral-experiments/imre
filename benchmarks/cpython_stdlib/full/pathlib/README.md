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

A null reason denotes a CPython pass. The script compares the oracle package
byte for byte with the vendored sources, independently loads all IDs using the
package hook, and checks that regrtest executed exactly those IDs. It enables
default-encoding warnings and uses a short temporary directory for Unix
sockets. All 28 Windows join tests and 52 of the 60 `PureWindowsPathTest`
methods run on Linux. The concrete `WindowsPathTest` class skips on this host.

This file records reference execution only. The existing
[`native_paths.py`](../../native_paths.py) separately compiles seven unchanged
method bodies with typed fixture adapters. A passing reference row alone says
nothing about whether Imre implements the operation.
