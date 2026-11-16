# Linux UtimeTests

This benchmark runs CPython 3.14.5's 13 original `test.test_os.UtimeTests` registrations under native unittest. They occupy positions 323–335 in the 375-case Linux reference inventory, each at occurrence 1.

| Position | Registration | CPython 3.14.5 | Imre on Linux |
| --- | --- | --- | --- |
| 323 | `test_issue31577` | Pass | Skipped by `support.cpython_only` |
| 324 | `test_large_time` | Skipped, requires NTFS | Skipped, requires NTFS |
| 325–335 | All eleven `test_utime*` registrations | Pass | Pass |

The class run uses the original `setUp`, file creation, cleanup, decorators and module teardown. It covers path and directory timestamps, file descriptors, directory-relative paths, the no-follow argument on a regular file, seconds and nanoseconds, the current time, missing files and invalid arguments. The full-module import on public Imre `aa1c3a37ed86` stops at `import fractions` before discovery. The table records the class-scoped run.

## Replay the historical explicit build

From the repository root, set `CPYTHON` to a test-enabled Linux CPython 3.14.5 interpreter, `IMRE` to the compiler, `RUNTIME` to its matching release `libimre_runtime.a` and `CLANG` to Clang 18. Use absolute paths. The staging command verifies vendored Git blobs against the pinned CPython manifests and retains the original source lines for the class and its dependencies.

```sh
work=$(mktemp -d)
python3 benchmarks/cpython_stdlib/full/os/stage_utime.py "$work/Lib"
mkdir "$work/reference"
(cd "$work/reference" && "$CPYTHON" -B "$work/Lib/loader.py")
for level in 0 3; do
    "$IMRE" build "$work/Lib/loader.py" --module-root "$work/Lib" \
        --runtime-imports --no-cache --opt-level "$level" \
        --runtime "$RUNTIME" --clang "$CLANG" -o "$work/utime-O$level"
    mkdir "$work/run-O$level"
    (cd "$work/run-O$level" && "$work/utime-O$level")
done
```

The entry accepts `list`, a method name or a full original ID. It prints the registrations, selections and unittest result for each run. To attempt the intact module, stage into a second new directory with `--whole-module`.

Run each negative control in a fresh writable directory. Each calls the real `os.utime` with a changed timestamp or file target and must fail one original assertion. These runs contribute zero original passes.

| Arguments to the executable | Changed observation |
| --- | --- |
| `lose-one-ns test_utime` | Subtract one nanosecond from access time |
| `wrong-mtime test_utime_by_times` | Add one second to modification time |
| `wrong-fd test_utime_fd` | Update a separate open file |
| `wrong-dir-fd test_utime_dir_fd` | Resolve the same name in the working directory |
