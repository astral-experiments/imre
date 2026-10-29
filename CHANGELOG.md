# Changelog

## 0.1.4

Released on 2026-10-29.

- Prevent out-of-bounds reads on truncated datagrams and exposure of uninitialized
  bytes on Linux TCP sockets when receiving with `MSG_TRUNC`.
  ([#2875](https://github.com/astral-experiments/imre/pull/2875),
  [#3005](https://github.com/astral-experiments/imre/pull/3005))
- Raise a catchable `MemoryError` when Imre cannot allocate path components or
  bind function arguments. If recording a traceback runs out of memory, the
  original exception remains available through `MemoryError.__context__`.
  ([#2975](https://github.com/astral-experiments/imre/pull/2975),
  [#3038](https://github.com/astral-experiments/imre/pull/3038),
  [#2467](https://github.com/astral-experiments/imre/pull/2467))
- Avoid native stack overflows when `ast.parse()` converts nested statements,
  when hashing deeply nested collection keys, and when runtime-loaded generators
  resume one another.
  ([#2936](https://github.com/astral-experiments/imre/pull/2936),
  [#2963](https://github.com/astral-experiments/imre/pull/2963),
  [#3033](https://github.com/astral-experiments/imre/pull/3033))
- Import `difflib` and `textwrap` in runtime-loaded Python modules. On POSIX,
  those modules can also wait for file descriptors with `select.select()` or
  `select.poll()`.
  ([#2488](https://github.com/astral-experiments/imre/pull/2488),
  [#2797](https://github.com/astral-experiments/imre/pull/2797),
  [#2776](https://github.com/astral-experiments/imre/pull/2776))
- Use mapping patterns, keyword class patterns such as `case Point(x=value)`,
  and generator expressions with multiple `for` clauses in runtime-loaded
  Python modules.
  ([#2442](https://github.com/astral-experiments/imre/pull/2442),
  [#2446](https://github.com/astral-experiments/imre/pull/2446),
  [#2431](https://github.com/astral-experiments/imre/pull/2431))
- Stream JSON to Python text writers with `json.dump()` in runtime-loaded
  modules, and enforce the configured integer digit limit when decoding JSON.
  ([#2794](https://github.com/astral-experiments/imre/pull/2794),
  [#2805](https://github.com/astral-experiments/imre/pull/2805))
- Speed up native container access and decimal integer parsing. Across two
  pinned Linux benchmark runs, median runtime fell 12% for telemetry and 16%
  for Base45.
  ([#2694](https://github.com/astral-experiments/imre/pull/2694))

[Full history for 0.1.4](https://github.com/astral-experiments/imre/compare/0.1.3...0.1.4)

## 0.1.3

Released on 2026-10-27.

- Relar is now Imre. The compiler executable is `imre`, and compiled programs
  report `imre` as their Python implementation.
  ([#397](https://github.com/astral-experiments/imre/pull/397))
- Run a script with `imre hello.py` or select Imre from uv with
  `uv run --python imre hello.py`. Both compile the script to native code.
  ([#302](https://github.com/astral-experiments/imre/pull/302))
- Export the native extension SDK with `imre build-target --output-dir sdk`.
  The SDK includes the header, wheel tag, and build adapters that setuptools
  and scikit-build-core need to build extensions for Imre.
  ([#274](https://github.com/astral-experiments/imre/pull/274))
- Use `collections.ChainMap` and `Counter.most_common()` in runtime-loaded
  modules. Look up an installed distribution’s version with
  `importlib.metadata.version()`.
  ([#2365](https://github.com/astral-experiments/imre/pull/2365),
  [#2364](https://github.com/astral-experiments/imre/pull/2364),
  [#2107](https://github.com/astral-experiments/imre/pull/2107))

[Full history for 0.1.3](https://github.com/astral-experiments/imre/compare/0.1.2...0.1.3)

## 0.1.2

Released on 2026-10-23.

### Enhancements

- Build and run programs on Linux x86-64 and ARM64 without installing a system
  compiler, linker, or development files. The release archives include Clang 18,
  LLD 18, and their native link inputs. This adds about 76 MB to each download.
  Use `--clang` to choose your own toolchain.
  ([#138](https://github.com/astral-sh/imre/pull/138))
- Compile Ruff’s complete `scripts/conformance.py` unchanged. Its command line
  runs both ty analyzers, decodes their JSON diagnostics into dataclasses, and
  renders both report formats byte-for-byte identically to Ruff on CPython.
  ([#130](https://github.com/astral-sh/imre/pull/130),
  [#132](https://github.com/astral-sh/imre/pull/132),
  [#135](https://github.com/astral-sh/imre/pull/135),
  [#137](https://github.com/astral-sh/imre/pull/137))

### Performance

- Relink native Linux x86-64 executables with LLD. Rebuilding Ruff’s seven-module
  reporter after a renderer edit took 0.88 s with LLD and 1.22 s with GNU BFD
  on the same pinned core. Switching linkers still reuses compiled module objects.
  ([#131](https://github.com/astral-sh/imre/pull/131))
- Vectorize more integer-list reductions when the element arithmetic and subtotal
  fit in machine integers. Summing `x * (x + 1)` over 4,096 digits, including
  parsing them from text, ran about twice as fast on x86-64.
  ([#134](https://github.com/astral-sh/imre/pull/134))
- Combine adjacent integer sums over the same `zip(strict=True)` into one
  traversal when the terms are total integer expressions. The telemetry reporter
  can compute signed, absolute, and squared error in one pass over its inputs.
  ([#139](https://github.com/astral-sh/imre/pull/139))
- Reuse uniquely owned record storage in loops that replace a record with a
  simple scalar-field constructor. A surviving alias keeps the original record.
  ([#145](https://github.com/astral-sh/imre/pull/145))

### Bug fixes

- Remove the `imre run` executable after its loader opens it and before user
  code starts. Killing the compiler and program together at that point leaves
  the temporary directory empty. Older explicitly supplied runtimes still clean
  up on normal exit.
  ([#147](https://github.com/astral-sh/imre/pull/147))
- Match CPython’s counted regex repetitions with an omitted minimum, including
  greedy, lazy, and unbounded forms.
  ([#127](https://github.com/astral-sh/imre/pull/127))
- Report diagnostics for excessively deep first-party imports, long `elif`
  chains, nested context managers, and `finally` blocks whose repeated cleanup
  would generate excessive LLVM IR.
  ([#149](https://github.com/astral-sh/imre/pull/149),
  [#152](https://github.com/astral-sh/imre/pull/152),
  [#156](https://github.com/astral-sh/imre/pull/156),
  [#160](https://github.com/astral-sh/imre/pull/160))
- Check long call chains and deeply nested class-field ownership graphs without
  exhausting the compiler’s native stack. Cycles still produce diagnostics.
  ([#162](https://github.com/astral-sh/imre/pull/162),
  [#164](https://github.com/astral-sh/imre/pull/164))

[Full history for 0.1.2](https://github.com/astral-sh/imre/compare/0.1.1...0.1.2)

## 0.1.1

Released on 2026-10-23.

### Enhancements

- Default `Path.read_text()` to UTF-8 regardless of the system locale.
  ([#121](https://github.com/astral-sh/imre/pull/121))
- Add booleans to integer counters, format signed integers with `f"{delta:+}"`,
  and infer an empty list’s scalar element type from `append`. These changes let
  Ruff’s per-file statistics table compile unchanged.
  ([#115](https://github.com/astral-sh/imre/pull/115))
- Run Ruff’s original annotation collector in the live conformance reporter. It
  reads each test file once before starting either ty process. On the 146-file
  checkout, the complete report matched Ruff on CPython byte for byte and took
  1.02× Rust’s elapsed time with the default allocator.
  ([#121](https://github.com/astral-sh/imre/pull/121))

### Performance

- Reuse native executables and module objects across `imre build` and
  `imre run` on x86-64 Debian and Ubuntu with Clang 18 and GNU BFD. An
  unchanged build of the seven-module Ruff reporter fell from 1.81 s to 15 ms.
  Editing its renderer recompiled one module and reused the other six.
  ([#120](https://github.com/astral-sh/imre/pull/120))
- Discard unused runtime code when linking. An unstripped Fibonacci executable
  on Apple Silicon shrank from 6.29 MiB to 815 KiB. Linux executables now use
  full RELRO by default. Use `--verbose-link` to inspect the linker command.
  ([#119](https://github.com/astral-sh/imre/pull/119))
- Vectorize sums of squares when Imre can prove that the inputs and subtotal
  fit in machine integers. Summing the squares of 4,096 digits in an existing
  list ran 21× faster on x86-64-v3, or 2.25× faster when the program also parsed
  the digits from text. ([#124](https://github.com/astral-sh/imre/pull/124))

### Bug fixes

- Reject malformed TOML integers and line breaks around `=`, including in fields
  the schema ignores. Accept a leading UTF-8 byte order mark in the CPython TOML
  decoder. ([#114](https://github.com/astral-sh/imre/pull/114),
  [#116](https://github.com/astral-sh/imre/pull/116),
  [#123](https://github.com/astral-sh/imre/pull/123))
- Match the literal hyphen after a regex range, as in `[a-b-c]`.
  ([#109](https://github.com/astral-sh/imre/pull/109))
- Report a compiler error for excessively deep expressions and long operator
  chains before indexing the module.
  ([#111](https://github.com/astral-sh/imre/pull/111))
- Detect when the executable and LLVM output point to the same file through a
  dangling symlink, even before the output exists.
  ([#107](https://github.com/astral-sh/imre/pull/107))

[Full history for 0.1.1](https://github.com/astral-sh/imre/compare/0.1.0...0.1.1)

## 0.1.0

Released on 2026-10-23.

Imre’s first release compiles a statically typed subset of Python to native
executables. Compiled programs run without a Python installation and retain
Python’s arbitrary-precision integers.

### Enhancements

- Build a standalone executable with `imre build app.py`, or compile and run it
  in one step with `imre run app.py`. Both commands follow static imports
  across first-party modules. ([#2](https://github.com/astral-sh/imre/pull/2),
  [#6](https://github.com/astral-sh/imre/pull/6),
  [#73](https://github.com/astral-sh/imre/pull/73),
  [#97](https://github.com/astral-sh/imre/pull/97))
- Define fixed-layout classes and dataclasses with methods and read-only
  properties. Declare string enums with named members and aliases.
  ([#7](https://github.com/astral-sh/imre/pull/7),
  [#67](https://github.com/astral-sh/imre/pull/67),
  [#93](https://github.com/astral-sh/imre/pull/93))
- Call functions with keyword arguments and scalar defaults. Pass typed
  functions, bound methods, and noncapturing lambdas as callbacks.
  ([#33](https://github.com/astral-sh/imre/pull/33),
  [#61](https://github.com/astral-sh/imre/pull/61))
- Write comprehensions over typed lists, dictionaries, and sets. Iterate with
  `zip(strict=True)` and `reversed`, and short-circuit generator expressions
  with `any` and `all`. ([#15](https://github.com/astral-sh/imre/pull/15),
  [#16](https://github.com/astral-sh/imre/pull/16),
  [#25](https://github.com/astral-sh/imre/pull/25),
  [#57](https://github.com/astral-sh/imre/pull/57))
- Match records, mappings, and sequences with `match`. Slice strings and lists,
  unpack sequences in assignments, and expand list and set literals with `*` and
  dictionary literals with `**`.
  ([#35](https://github.com/astral-sh/imre/pull/35),
  [#39](https://github.com/astral-sh/imre/pull/39),
  [#45](https://github.com/astral-sh/imre/pull/45))
- Catch built-in exceptions such as `ValueError` and `OSError` and run cleanup
  in `finally`. ([#18](https://github.com/astral-sh/imre/pull/18),
  [#21](https://github.com/astral-sh/imre/pull/21))
- Decode JSON and TOML into typed records and containers, and serialize typed
  values back to JSON. ([#63](https://github.com/astral-sh/imre/pull/63),
  [#81](https://github.com/astral-sh/imre/pull/81),
  [#84](https://github.com/astral-sh/imre/pull/84))
- Search text with fixed regular expressions and extract named capture groups.
  ([#92](https://github.com/astral-sh/imre/pull/92))
- Read plain or gzip files line by line, write files atomically, and run
  subprocesses with captured output and a timeout.
  ([#71](https://github.com/astral-sh/imre/pull/71),
  [#79](https://github.com/astral-sh/imre/pull/79),
  [#84](https://github.com/astral-sh/imre/pull/84),
  [#87](https://github.com/astral-sh/imre/pull/87))
- Compile the upstream `python-base45` codec and `luhn` checksum implementations
  unchanged, including Luhn’s unannotated functions.
  ([#48](https://github.com/astral-sh/imre/pull/48),
  [#53](https://github.com/astral-sh/imre/pull/53))
- Run typed ports of uv’s Python download catalog, Ruff’s typing conformance
  reporter, and the memory-report comparator used for ty. Ruff’s diagnostic
  collector, grouping, and per-file statistics functions compile unchanged.
  ([#64](https://github.com/astral-sh/imre/pull/64),
  [#82](https://github.com/astral-sh/imre/pull/82),
  [#95](https://github.com/astral-sh/imre/pull/95),
  [#102](https://github.com/astral-sh/imre/pull/102))

### Performance

- Reuse string buffers when encoding Base45 and fuse the digit reductions in
  Luhn’s checksum. ([#49](https://github.com/astral-sh/imre/pull/49),
  [#55](https://github.com/astral-sh/imre/pull/55))
- Reduce record and string allocations in uv’s download catalog. Across plain
  and gzip inputs at the original size and at ten times that size, Imre took
  1.11–1.22× Rust’s elapsed time.
  ([#90](https://github.com/astral-sh/imre/pull/90),
  [#91](https://github.com/astral-sh/imre/pull/91))
- Select jemalloc on Linux with `--allocator jemalloc`. Grouping Ruff
  diagnostics for 1,460 files took 1.07–1.17× Rust’s elapsed time with jemalloc
  on both sides. ([#102](https://github.com/astral-sh/imre/pull/102))

### Release

- Install Imre from Astral Labs on x86-64 Linux, ARM64 Linux, and Apple Silicon
  macOS. The shell installer installs the compiler and runtime together.
  ([#101](https://github.com/astral-sh/imre/pull/101),
  [#103](https://github.com/astral-sh/imre/pull/103),
  [#104](https://github.com/astral-sh/imre/pull/104))

[Full history for 0.1.0](https://github.com/astral-sh/imre/commits/0974aad73cbc0d909a80e8bbe8c23d9f028ce10e/)
