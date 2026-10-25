# CPython itertools evidence

| Original suite | Linux reference | Native evidence, both optimization levels |
| --- | --- | --- |
| test.test_itertools | [137 occurrences](reference-linux.json): 136 unittest methods and the batched doctest | [evidence-linux.json](evidence-linux.json) |
| test.test_free_threading.test_itertools_batched | [1 occurrence](reference-free-threading-linux.json) on free-threaded CPython | [evidence-free-threading-linux.json](evidence-free-threading-linux.json) |

These are separate upstream suites, for **138 occurrences** in total. The JSON
doctests are part of JSON's denominator. Imre's LLVM optimization level does
not change CPython's discovery or remove docstrings.

At O0 and O3, eleven complete primary methods pass under their original host
fixtures. The 106 blocked occurrences, four reference skips and sixteen
reviewed exclusions remain in the primary report. The companion is blocked
at both levels.

Re-running all 137 primary occurrences on the tuple-display compiler after
StringIO #928 kept every classification at O0 and O3. Three first diagnostics
changed: the filter method now reaches `pickle` after `copy`; the groupby
method reaches the restriction on how a group can be consumed; and the GC
count method stops while creating a subclass of `int`. With the merged
Template and mapping support, the latter advances from the keyword in
`dict(x=a)` to the enclosing three-argument `type` call, before the first
`itertools.count` operation. Its role and outcome remain unchanged. The
free-threading result is unchanged. The [four whole-file startup checks](tuple-spread-controls-linux.json)
are separate from these method results.

[coverage.py](coverage.py) copies complete original test methods and the
original global definitions and self helpers they reference. It uses the
[shared runner](../../SUITE_RUNNER.md) to run the original unittest lifecycle.
Every credited method records its source hash, native operations and native
assertions. The coordinator's [v1 contract](../../NATIVE_EVIDENCE.md) classifies
these bodies as `native-body-host-fixture`: CPython dispatches the original
fixtures, and an Imre subprocess constructs its native instance. Filter and
zip are builtin-only examples inside the upstream denominator.

On the reference build, four huge allocations skip under the upstream
address-space rule. The default resource configuration also skips the long-chain
regression. That unchanged method passes with `-u cpu`; the native attempt
runs with CPU enabled and records its own blocker. The original default
reference outcome is retained. Each of six CPython tuple-reuse checks, six
CPython tuple-GC-tracking checks and four exact C object-size checks has its
own source, fixture, reason and revisit condition. All 24 ordinary TestGC
methods remain in the workload.

The [batched example](upstream/batched.doctest) comes verbatim from
[the original C docstring](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Modules/itertoolsmodule.c#L124).
The runner verifies the blob and parses the immutable example before compiling
it. Its output comparison, when reached, uses CPython's OutputChecker and is
reported separately from the native iterator operation.

The [companion](upstream/Lib/test/test_free_threading/test_itertools_batched.py)
starts 10 threads against one batched iterator, 20 times. Its
[package guard](upstream/Lib/test/test_free_threading/__init__.py) raises
`SkipTest("GIL enabled")` before discovery on ordinary CPython.
[free_threading.py](free_threading.py) discovers and runs the method with a
free-threaded build of the exact release, then attempts the whole source
(including its decorator and thread fixture) in Imre. This keeps the single
companion occurrence visible without counting the two CPython configurations twice.

From the repo root, set `CPYTHON` to a test-enabled CPython 3.14.5 and
`CPYTHON_T` to a free-threaded build from the same commit:

```sh
dir=benchmarks/cpython_stdlib/full/itertools
"$CPYTHON" -B "$dir/coverage.py" --imre "$IMRE" --runtime "$RUNTIME" --clang "$CLANG" \
    --cpython-source "$CPYTHON_SOURCE" --baseline "$dir/evidence-linux.json" --result /tmp/itertools-evidence.json
"$CPYTHON_T" -B "$dir/free_threading.py" --imre "$IMRE" --runtime "$RUNTIME" --clang "$CLANG" \
    --gil-python "$CPYTHON" --result /tmp/itertools-free-threading-evidence.json
python3 benchmarks/cpython_stdlib/suite_evidence.py "$dir/reference-linux.json" /tmp/itertools-evidence.json
python3 benchmarks/cpython_stdlib/suite_evidence.py "$dir/reference-free-threading-linux.json" /tmp/itertools-free-threading-evidence.json
```
