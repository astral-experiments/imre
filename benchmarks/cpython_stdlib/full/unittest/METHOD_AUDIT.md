# Auditing all unittest methods

[projection_audit.py](projection_audit.py) finds compiler and runtime blockers for the
unchanged methods in the 3.14.5 package. The input is the same package discovery and
[Linux reference](reference-linux.json) as the suite runner. Every report includes
all 1,090 occurrences, including repeated IDs and the original SIGINT skips.

From the repository root, using a test-enabled CPython 3.14.5:

```sh
/path/to/python -B benchmarks/cpython_stdlib/full/unittest/projection_audit.py \
  --cpython-source /path/to/cpython --compiler-commit FULL_IMRE_REVISION \
  --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
  --jobs 2 --result /tmp/unittest-methods.json
```

The compiler, runtime, method byte ranges and generated programs are identified by
SHA-256. Before building, the audit checks the 38 vendored unittest files, the
interpreter's copies, and all 1,639 original `Lib/test` fixture files. Their names,
modes and bytes must reconstruct CPython's [pinned Git tree](https://github.com/python/cpython/tree/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test).

Each projection copies an entire method, its directly used instance helpers and its
instance setup and teardown from those verified files. It supplies a new class and
driver and selects the imports those definitions use. It does not recreate the
original class identity, class or module setup, cleanup stack, decorators or runner.
A native return therefore identifies a candidate for the [fixture runner](../../suite_native.py);
it does not count as a native test pass. The audit does not change the
[native evidence contract](../../NATIVE_EVIDENCE.md).

Each occurrence records the first diagnostic, its upstream line when known, or the
native exit and comparison with the same driver under CPython. There are four separate
observations: ordinary and reachable compilation at each of O0 and O3. Async or
generator methods which a direct call would only construct retain their exact
source blocker. The two reference skips retain their reasons. Repeated `--smoke ID`
selects individual methods; unscheduled occurrences remain in the report. A
misspelled ID is an error.

## Recorded Linux run

The [compressed receipt](projection-linux.json.gz) contains every occurrence, its
source hash, and all four observations from [compiler bf141eb](https://github.com/astral-sh/imre/commit/bf141ebbc22492f9d7d8f530cab57f1cae23232a).
This compiler includes the proposed callable and regex exception assertions.
The uncompressed JSON has SHA-256
`68f46b7f6e609a6436b3a3f78a12f7ef42fb4359c5982518223be9d4c6bd595d`.
All four modes found the same counts:

| Outcome | Occurrences per mode |
| --- | ---: |
| Projected method returned | 12 |
| Compiler reported a blocker | 1,024 |
| Fixture or method could not be projected | 52 |
| Original Linux reference skip | 2 |
| Total | **1,090** |

For example, `Test_TestCase.testAssertEqualSingleLine` returned in all four
modes. `Test_TestCase.testAssertMultiLineEqualTruncates` reached the call that
the native method rejects. The receipt maps both results to the original method
definitions. Use `gzip -dc projection-linux.json.gz` to inspect them.
