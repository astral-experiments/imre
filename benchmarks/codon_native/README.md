# Native Codon comparison

`build.py` prepares the same application algorithms for the pinned official
Codon 0.20.4 Linux release. It retains source and generated-source hashes and
records every declaration, import, integer-conversion, and keyed-sort adaptation.
Python's `slots`, `kw_only`, and `frozen` declaration options become native
fixed-field dataclasses. Bare `*,`
keyword-only declaration markers are removed; keyword calls remain unchanged.
Existing one-argument `int` calls redirect to a native helper that normalizes
Unicode decimal digits and non-ASCII whitespace before Codon's signed-integer
parser. ASCII strings use a small whitespace guard and the native
parser; boolean conversions pass through directly. The guard also rejects
ASCII control separators U+001C–U+001F that Python's `int` does not accept.
Calls to `sorted(..., key=...)` use a native helper that materializes the input,
computes every key once in input order, and sorts decorated entries with an
ordinal tie breaker. This preserves eager key errors for single-item inputs and
stable ties in both sort directions; stock Codon calls keys during comparisons.
The Ruff diagnostic-entry collector uses stock Codon's keyed in-place list sort.
Its key reads a priority and two string fields. The benchmark verifies every
ordered title, case key, and source against Ruff. Codon supplies an
`AssertionError` for the collector's unreachable `typing.assert_never` branches.
The builder retains both original and generated function-body hashes and checks
that those declared call replacements are the only function-body AST changes.
Generated modules use `.codon` filenames and an explicit `CODON_PATH` for Codon's
package search. Typed JSON adapters use Codon's stock native JSON parser/formatter.
A native UTF-8 percent decoder supplies the bounded
`urllib.parse.unquote` operation.

The conformance reporter uses named captures from Ruff’s verbose annotation
pattern. Codon 0.20.4’s `re` module accepts a `VERBOSE` argument but misses
markers in that pattern, and its module-level `search` expects a string pattern.
The benchmark adapter checks the declaration against the vendored Ruff source,
removes verbose layout, spells out Python’s whitespace set, and calls the native
RE2 pattern’s `search` method. Capture text and character offsets are checked
against Ruff on CPython.

Codon 0.20.4's formatter leaves DEL (U+007F) literal with `ensure_ascii=True`.
The encoding adapter normalizes that character after formatting, including
the extra output scan in measured work. All other formatting uses stock JSON.

Builds use `-release -numerics=py -disable-native`. There is no Python fallback,
CPython extension, auto-python mode, fast-math, or unordered-dictionary option.
Run validation with `CODON_PYTHON` pointing to a nonexistent library. Resulting
executables depend on the pinned native Codon runtime libraries.

Comparisons use the shared valid-input domain. Codon integers are signed 64-bit;
Imre's arbitrary-precision fixtures remain separately tested. Codon's stock
JSON parser retains the last duplicate key, whereas Imre's typed decoder
rejects duplicate known fields. Record these unsupported validation cases
explicitly; do not count them as passing. All engines must still reproduce the
complete real program output and filesystem effects on shared-domain inputs.

The [official release](https://github.com/exaloop/codon/releases/tag/v0.20.4)
contains LLVM 20.1.7 and uses Apache-2.0 for Codon's source, with separate
third-party runtime components. The Linux archive SHA-256 is
`490d2263e2a543b7786e8f5898eb767dd6a67945238c90fe8be130b4331eb8c9`.
The builder verifies the compiler and both Codon shared libraries against the
official release identities; it records the native JSON module hash as well.

The Python download catalog uses Codon's native plain and gzip file readers.
The builder removes the CPython `TextIO` parameter annotation so Codon can
specialize the same reader body for both handle types. The build records the
identities of Codon's gzip and file modules. Its gzip reader accepts
uncompressed input transparently and stops at zero padding between members.
The differential suite records these two differences. The measured archive
contains ordinary, adjacent gzip members and every engine must read them all.

```sh
python3 benchmarks/codon_native/build.py \
  --codon /path/to/codon-deploy-linux-x86_64/bin/codon \
  --platform-library /path/to/native-platform/libimre_benchmark_platform.so \
  --output /path/to/fresh-build-directory
```

## Live conformance workflow

The checkout-based reporter additionally uses explicit native TOML, subprocess,
and temporary-directory adapters. Build `benchmarks/native_platform` and pass
its shared library with `--platform-library`; select `--applications conformance_report`
to build the live and compute reporter binaries. The platform library uses Rust
for configuration decoding, deadline-bound process capture, and temporary
directories. It contains no annotation parsing, diagnostic classification, or
report rendering, and links neither Python nor Imre.

The subprocess adapter serializes arguments and the environment across its native
FFI boundary and returns decoded captures. All serialization and native calls
remain inside live timings. The Rust reference calls the same platform routines
through Rust types. A `__fspath__` adapter accommodates Codon's context manager
binding behavior; native error adapters preserve the tested exception categories
and cleanup, although Codon's uncaught-error process exit differs from Python's.

Ordinary and ThinLTO comparisons keep the Codon compiler flags above. Only its
linked Rust platform library changes between those pairings; this is not a
Codon cross-language ThinLTO build. The compute reporter has no platform calls.

For the conformance reporter, Codon 0.20.4 supplies a native singleton `Enum`.
The adapter spells out Ruff’s lowercase `auto()` values, returns the string
payload from `__str__`, and uses `str` for the title method's closed verb
annotation. Member comparisons, value patterns, classification, statistics, and
title methods use the application’s original function bodies. The classification
comparison checks both source versions, all four classifications, all four
change kinds, and the resulting Markdown against upstream CPython.

The grouping benchmark specializes the upstream `Sequence[Path]` annotation
to the list provided by all callers. Codon's native `defaultdict` infers its
factory type from a concrete `list[TyDiagnostic]` or `list[ExpectedError]`.
Its dictionary keys are iterators, so the adapter materializes three key sets
for the union. A non-null tag is passed through `str` after the original
filter. Native `Path` handles file resolution and basename grouping. The
comparison checks the order and fields of every diagnostic and expected error,
each group's equivalence, and the per-file counts in source-list order.

The diagnostic section uses Codon's native `groupby`, list concatenation,
generator `min` and `max`, and record `__str__`. The two calls to the HTML row
renderer pass the selected checkout URLs explicitly so its sibling function can
use them. Codon 0.20.4 accepts named fields through `str.format_map`. The
adapter puts the filename and integer line into a native `dict[str, str | int]`
and calls `format_map` on the original template.

The conformance summary uses a native `textwrap.dedent` adapter. It removes
the common space/tab prefix after interpolation and normalizes whitespace-only
lines. For Ruff’s zero-true-positive error, the builder gives six record types
Python-style `__repr__` methods and raises `AssertionError` explicitly so its
message contains exactly the first five grouped cases. Codon’s uncaught error
handler prints its source location in a separate traceback.
