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
