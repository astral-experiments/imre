# Linux zero-copy operations

[The two optimization reports](fastcopy-linux.json) retain all 29 inherited and class-specific occurrences in `TestZeroCopySendfile` and `TestZeroCopyCopyFileRange`. At each optimization level, the original CPython bodies and assertions observed 8 cases with native module operations, 17 with mixed operations, and 4 blocked. None is a fully native method. The reports log 28 native module calls, 15 calls to original host mocks, and 4 calls to CPython's stream fallback at each level.

[The fixture](fastcopy_ops.py) passes the original open descriptions into an Imre child. The child uses `sendfile` or `copy_file_range` and returns the caught exception's own `errno`, `strerror`, `filename`, and `filename2`. Both missing-source cases produce ENOENT with their original input and no second name. The two partially completed copies produce EBADF with both filenames; the two full-filesystem mocks produce ENOSPC with both filenames and suppressed context. A syscall mock's custom Python message (`"yo"`) does not travel through POSIX errno. Those rows remain mixed. The tests that raise `ZeroDivisionError` at a mocked syscall remain blocked, including the block-size checks that depend on that exception arriving at the caller.

Run with the pinned, test-enabled CPython 3.14.5, an Imre compiler, and its matching runtime:

```sh
"$PY314" -B benchmarks/cpython_stdlib/full/shutil/fastcopy_ops.py \
    --imre "$IMRE" --runtime "$RUNTIME" --clang "$CLANG" --result /tmp/fastcopy-linux.json
"$PY314" -B benchmarks/cpython_stdlib/full/shutil/fastcopy_controls.py \
    --imre "$IMRE" --runtime "$RUNTIME" --clang "$CLANG"
```

The [supplemental controls](fastcopy_controls.py) compare real descriptor offsets, empty and nonempty transfers, public feature flags, and catch ordering against CPython. `_GiveupOnFastCopy` (kind 69) wraps an actual `io.UnsupportedOperation` when an in-memory file has no descriptor. CSV, Unicode, and shutil handlers must not catch that fallback, and its own handler must not catch unrelated CSV, Unicode, or same-file errors. These controls contribute no additional test IDs.
