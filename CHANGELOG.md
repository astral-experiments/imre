# Changelog

## 0.1.1

Released on 2026-10-23.

### Enhancements

- Default `Path.read_text()` to UTF-8 regardless of the system locale.
  ([#121](https://github.com/astral-sh/relar/pull/121))
- Add booleans to integer counters, format signed integers with `f"{delta:+}"`,
  and infer an empty list’s scalar element type from `append`. These changes let
  Ruff’s per-file statistics table compile unchanged.
  ([#115](https://github.com/astral-sh/relar/pull/115))
- Run Ruff’s original annotation collector in the live conformance reporter. It
  reads each test file once before starting either ty process. On the 146-file
  checkout, the complete report matched Ruff on CPython byte for byte and took
  1.02× Rust’s elapsed time with the default allocator.
  ([#121](https://github.com/astral-sh/relar/pull/121))

### Performance

- Reuse native executables and module objects across `relar build` and
  `relar run` on x86-64 Debian and Ubuntu with Clang 18 and GNU BFD. An
  unchanged build of the seven-module Ruff reporter fell from 1.81 s to 15 ms.
  Editing its renderer recompiled one module and reused the other six.
  ([#120](https://github.com/astral-sh/relar/pull/120))
- Discard unused runtime code when linking. An unstripped Fibonacci executable
  on Apple Silicon shrank from 6.29 MiB to 815 KiB. Linux executables now use
  full RELRO by default. Use `--verbose-link` to inspect the linker command.
  ([#119](https://github.com/astral-sh/relar/pull/119))
- Vectorize sums of squares when Relar can prove that the inputs and subtotal
  fit in machine integers. Summing the squares of 4,096 digits in an existing
  list ran 21× faster on x86-64-v3, or 2.25× faster when the program also parsed
  the digits from text. ([#124](https://github.com/astral-sh/relar/pull/124))

### Bug fixes

- Reject malformed TOML integers and line breaks around `=`, including in fields
  the schema ignores. Accept a leading UTF-8 byte order mark in the CPython TOML
  decoder. ([#114](https://github.com/astral-sh/relar/pull/114),
  [#116](https://github.com/astral-sh/relar/pull/116),
  [#123](https://github.com/astral-sh/relar/pull/123))
- Match the literal hyphen after a regex range, as in `[a-b-c]`.
  ([#109](https://github.com/astral-sh/relar/pull/109))
- Report a compiler error for excessively deep expressions and long operator
  chains before indexing the module.
  ([#111](https://github.com/astral-sh/relar/pull/111))
- Detect when the executable and LLVM output point to the same file through a
  dangling symlink, even before the output exists.
  ([#107](https://github.com/astral-sh/relar/pull/107))

[Full history for 0.1.1](https://github.com/astral-sh/relar/compare/0.1.0...0.1.1)

## 0.1.0

Released on 2026-10-23.

Relar’s first release compiles a statically typed subset of Python to native
executables. Compiled programs run without a Python installation and retain
Python’s arbitrary-precision integers.

### Enhancements

- Build a standalone executable with `relar build app.py`, or compile and run it
  in one step with `relar run app.py`. Both commands follow static imports
  across first-party modules. ([#2](https://github.com/astral-sh/relar/pull/2),
  [#6](https://github.com/astral-sh/relar/pull/6),
  [#73](https://github.com/astral-sh/relar/pull/73),
  [#97](https://github.com/astral-sh/relar/pull/97))
- Define fixed-layout classes and dataclasses with methods and read-only
  properties. Declare string enums with named members and aliases.
  ([#7](https://github.com/astral-sh/relar/pull/7),
  [#67](https://github.com/astral-sh/relar/pull/67),
  [#93](https://github.com/astral-sh/relar/pull/93))
- Call functions with keyword arguments and scalar defaults. Pass typed
  functions, bound methods, and noncapturing lambdas as callbacks.
  ([#33](https://github.com/astral-sh/relar/pull/33),
  [#61](https://github.com/astral-sh/relar/pull/61))
- Write comprehensions over typed lists, dictionaries, and sets. Iterate with
  `zip(strict=True)` and `reversed`, and short-circuit generator expressions
  with `any` and `all`. ([#15](https://github.com/astral-sh/relar/pull/15),
  [#16](https://github.com/astral-sh/relar/pull/16),
  [#25](https://github.com/astral-sh/relar/pull/25),
  [#57](https://github.com/astral-sh/relar/pull/57))
- Match records, mappings, and sequences with `match`. Slice strings and lists,
  unpack sequences in assignments, and expand list and set literals with `*` and
  dictionary literals with `**`.
  ([#35](https://github.com/astral-sh/relar/pull/35),
  [#39](https://github.com/astral-sh/relar/pull/39),
  [#45](https://github.com/astral-sh/relar/pull/45))
- Catch built-in exceptions such as `ValueError` and `OSError` and run cleanup
  in `finally`. ([#18](https://github.com/astral-sh/relar/pull/18),
  [#21](https://github.com/astral-sh/relar/pull/21))
- Decode JSON and TOML into typed records and containers, and serialize typed
  values back to JSON. ([#63](https://github.com/astral-sh/relar/pull/63),
  [#81](https://github.com/astral-sh/relar/pull/81),
  [#84](https://github.com/astral-sh/relar/pull/84))
- Search text with fixed regular expressions and extract named capture groups.
  ([#92](https://github.com/astral-sh/relar/pull/92))
- Read plain or gzip files line by line, write files atomically, and run
  subprocesses with captured output and a timeout.
  ([#71](https://github.com/astral-sh/relar/pull/71),
  [#79](https://github.com/astral-sh/relar/pull/79),
  [#84](https://github.com/astral-sh/relar/pull/84),
  [#87](https://github.com/astral-sh/relar/pull/87))
- Compile the upstream `python-base45` codec and `luhn` checksum implementations
  unchanged, including Luhn’s unannotated functions.
  ([#48](https://github.com/astral-sh/relar/pull/48),
  [#53](https://github.com/astral-sh/relar/pull/53))
- Run typed ports of uv’s Python download catalog, Ruff’s typing conformance
  reporter, and the memory-report comparator used for ty. Ruff’s diagnostic
  collector, grouping, and per-file statistics functions compile unchanged.
  ([#64](https://github.com/astral-sh/relar/pull/64),
  [#82](https://github.com/astral-sh/relar/pull/82),
  [#95](https://github.com/astral-sh/relar/pull/95),
  [#102](https://github.com/astral-sh/relar/pull/102))

### Performance

- Reuse string buffers when encoding Base45 and fuse the digit reductions in
  Luhn’s checksum. ([#49](https://github.com/astral-sh/relar/pull/49),
  [#55](https://github.com/astral-sh/relar/pull/55))
- Reduce record and string allocations in uv’s download catalog. Across plain
  and gzip inputs at the original size and at ten times that size, Relar took
  1.11–1.22× Rust’s elapsed time.
  ([#90](https://github.com/astral-sh/relar/pull/90),
  [#91](https://github.com/astral-sh/relar/pull/91))
- Select jemalloc on Linux with `--allocator jemalloc`. Grouping Ruff
  diagnostics for 1,460 files took 1.07–1.17× Rust’s elapsed time with jemalloc
  on both sides. ([#102](https://github.com/astral-sh/relar/pull/102))

### Release

- Install Relar from Astral Labs on x86-64 Linux, ARM64 Linux, and Apple Silicon
  macOS. The shell installer installs the compiler and runtime together.
  ([#101](https://github.com/astral-sh/relar/pull/101),
  [#103](https://github.com/astral-sh/relar/pull/103),
  [#104](https://github.com/astral-sh/relar/pull/104))

[Full history for 0.1.0](https://github.com/astral-sh/relar/commits/0974aad73cbc0d909a80e8bbe8c23d9f028ce10e/)
