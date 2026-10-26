# Native argparse results

The [shared unittest runner](../../SUITE_RUNNER.md) runs the original module,
class and instance fixtures under the pinned CPython interpreter. For these
generated methods, the test body and assertions also run in CPython. The adapter records the declarations and whether the method actually
passes an argument list, builds the same program with Imre, and compares its
exit status, stdout and stderr against a separate CPython subprocess. Successful
assertions receive values decoded from the native process. CPython does not
supply values or implement actions in that process.

For source methods that expect an `add_argument` exception, the per-occurrence
[evidence runner](evidence.py) first attempts the complete native body. If the
body is blocked and the original method contains a supported registration
assertion, [the registration adapter](registration_cases.py) runs the call in
Imre. It compares the exception type and message, exit status, stdout, and
stderr with CPython, then carries the native exception back to the original
host assertion. The record keeps the whole-body diagnostic and credits only
the registration operation to Imre.

```sh
python3.14 -B benchmarks/cpython_stdlib/full/argparse/native_cases.py \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang clang-18 \
  --imre-revision "$(git rev-parse HEAD)" --result /tmp/argparse-native.json
```

The first measurement uses the compiler and runtime from [#491](https://github.com/astral-sh/imre/pull/491),
with the shared fixture runner from [#508](https://github.com/astral-sh/imre/pull/508).
Both levels account for the same 1,893 upstream IDs, independently.

| Execution | -O0 | -O3 |
|---|---:|---:|
| Fully native test bodies and fixtures | 0 | 0 |
| Native test bodies with host fixtures | 0 | 0 |
| Complete host-driven routines with native parser operations | 48 | 48 |
| Mixed host/native parser operations | 0 | 0 |
| Reference only | 669 | 669 |
| Blocked | 1,176 | 1,176 |
| Failed native comparisons or assertions | 0 | 0 |
| Reference skips or exclusions | 0 | 0 |

The 48 passing routines made 192 native parse calls per level. The adapter
targets 1,236 generated parsing routines. Twelve of those had no parse calls
in their upstream input tables; 657 other tests were only run in the reference
interpreter. A compiler or adapter diagnostic blocks the whole routine even
when some of its subtests work. See `native-linux.json` for the IDs, reasons
and optimization levels. The original reference suite passed every ID.

Compiling the whole upstream module directly fails the Imre syntax complexity
limit before imports or unittest discovery. The first parser-specific blocker
in this measurement is group declarations (632 IDs). Neither this report nor
the CLI fixture comparisons count a fully native unittest body.
