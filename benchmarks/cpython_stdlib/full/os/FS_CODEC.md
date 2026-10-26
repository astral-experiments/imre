# POSIX filesystem encoding

`os.fsdecode(filename)` returns a native string input by identity. It decodes a bytes input using the process's filesystem encoding and error handler. `os.fsencode(filename)` preserves bytes by identity and encodes native strings with the same settings. Both accept a pathlib path or a user object with `__fspath__`. The descriptor executes once, and its result or exception reaches the caller. The public fixture exercises keyword binding, NUL, UTF-8, nominal string enum identity, and invalid path types and results. [Native fixture](../../../../tests/fixtures/paths/filesystem_codec.py).

CPython 3.14.5 on Linux decodes the byte at index 7 of `b"actual-\xff.bin"` as U+DCFF and recovers all 256 byte values through `fsencode(fsdecode(raw))`. Imre can open this filename as bytes. Decoding it with the POSIX filesystem error handler raises `NotImplementedError`. Native `Str` uses `compact_str::CompactString` and exposes Rust `&str`, which represents Unicode scalar values. U+DC80 through U+DCFF require a different string representation throughout indexing, identity, comparisons and encoding. A separate compiler restriction rejects ordinary `class MyStr(str)` declarations before their path operations can run. [Native string representation](../../../../crates/imre-runtime/src/lib.rs) and [codec error handling](../../../../crates/imre-runtime/src/objects/codecs.rs).

With `LC_ALL=C`, `PYTHONUTF8=0` and `PYTHONCOERCECLOCALE=0`, both runtimes selected ASCII. They decoded `b"ascii"` and raised the same `UnicodeEncodeError` for the character at position 3 of `"café"`.

## Werkzeug 3.1.8

The original `FileStorage` ran in CPython from the seven verified Flask wheels. Its imported `fsdecode` and `fspath` bindings invoked compiled Imre programs. The programs reconstructed each typed operand, called the public OS function and returned its result, identity, exception and descriptor call count. CPython supplied the streams, temporary files and assertions.

| Observation at each of ordinary and reachable-only O0/O3 | Cases |
| --- | ---: |
| Matched CPython, including the no-name stream control | 24 |
| Needed surrogate-bearing strings | 7 |
| Blocked while compiling a plain `str` subclass | 4 |
| Total | 35 |

The wheel reached 38 filesystem call sites per mode. Imre executed 34 calls, including seven explicit representation errors. Four calls stopped at the class declaration. An implicit stream name of `b"<stdin>"` became `None`. An explicit filename with those same bytes became `"<stdin>"`. A save destination whose `__fspath__` returned bytes raised Werkzeug's original `AttributeError` from the missing `write` method. [Per-case observations and wheel hashes](filesystem-codec-linux.json).

The separate whole-application gate served all five requests on CPython. All four native builds reached the module-level starred tuple at `werkzeug/urls.py:54` and stopped before serving requests. [Reproduce the unchanged Flask gate](../../../flask/README.md).

Run the wheel comparison on Linux with the pinned CPython and wheelhouse. It writes all 35 observations and their actual wheel call sites for each mode. [Runner](werkzeug.py).

```sh
"$python314" -IB benchmarks/cpython_stdlib/full/os/werkzeug.py \
    --imre "$imre" --runtime "$runtime" --clang "$clang" \
    --wheelhouse "$wheelhouse" --output "$output"
```
