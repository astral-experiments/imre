# Timed file rotation

`native_timed.py` exercises twenty original `TimedRotatingFileHandlerTest`
IDs. One native driver retains all handlers for a method, starts under that
method's timezone, and uses its real temporary files. The host provides
unittest, datetime inputs, and assertions. Constructors, scheduling, deletion
selection, and close run in the Imre executable. The daily `atTime` method
also compiles verbatim with a filename fixture and native integer assertions.
Count that ID only once.

At `-O0` and `-O3`, the 18 `test_compute_rollover_*` methods,
`test_invalid`, and `test_compute_files_to_delete_same_filename_different_extensions`
passed. The host-driven route made 293 native calls at each optimization level.
A separate file comparison covers descriptor closure, immediate and delayed
opening, forced rotation, and retention of the latest backup.

Run with a test-enabled CPython 3.14.5:

```sh
/path/to/python -B benchmarks/cpython_stdlib/full/logging/native_timed.py \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a \
  --clang /path/to/clang-18 --opt-level 0
```

The public handler supports native UTF-8 files, `computeRollover`,
`shouldRollover`, `doRollover`, default backup naming and `getFilesToDelete`.
Calendar scheduling accepts a naive `datetime.time` and honors its hour,
minute and second. Record emission and custom naming callbacks need separate
coverage; none of these twenty IDs claims either.
