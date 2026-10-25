# CPython 3.14.5 `test_sys`

The upstream suite is a single [unmodified module](../../upstream/Lib/test/test_sys.py).
The [manifest](../../UPSTREAM.json) pins it and the [license](../../upstream/LICENSE)
to CPython commit `5607950ef232dad16d75c0cf53101d9649d89115`.

On Linux, discovery and the reference runner both returned **97 distinct IDs**.
[Reference results](reference-linux.json) record 89 passes and 8 upstream skips.
The remote debugger tests need to inspect their child processes; running regrtest in
a sandbox that hides the native executable mapping produces errors in ten of them.
Use a test-enabled checkout of the pinned release to reproduce the reference:

```sh
"$CPYTHON" -B -m test --single-process --randseed 1 --junit-xml /tmp/sys.xml test_sys
```

To run the selected full methods against the compiler and runtime under test, use
the [shared suite harness](../../SUITE_RUNNER.md). Module, class and instance
fixture hooks run in the pinned CPython host. Each result records where the
original body, its assertions and the tested operations execute.

```sh
for opt in 0 3; do
    "$CPYTHON" -X utf8 benchmarks/cpython_stdlib/full/sys/run.py \
        --imre "$IMRE" --runtime "$RUNTIME" --clang "$CLANG" \
        --opt-level "$opt" --result "/tmp/sys-O$opt.json"
done
python3 benchmarks/cpython_stdlib/full/sys/coverage.py --result /tmp/sys-coverage.json
```

- `test_sys_exception_no_exception`: the complete method queries native handler state and compares the result to `None`. A separate program checks object identity across nested handlers, calls and handler exit.
- `test_43581`: the complete method, both stream attributes, equality and assertion execute in the native binary.
- `test_getdefaultencoding` and `test_getfilesystemencoding`: all `sys` operations execute in the native driver. The original unittest assertions, codec lookup and transport run on CPython.
- `test_ioencoding_nonascii`: the original subprocess source executes as a native program. CPython owns the fixture, process pipes and expected filesystem bytes. The default Linux reference skips this test because the locale and filesystem codec names differ in case; `-X utf8` satisfies its unchanged decorator.

The existing [stream and exit smoke runner](../../native_sys.py) exercises four
`test_subprocess` IDs and additional native program fixtures. Those are outside this
suite's denominator. A CPython-only result or an operation probe is not a native
pass for a complete routine.

The coverage report joins [every reference ID](reference-linux.json) to the
two optimization reports and [individual assessments](assessments.tsv). It
rejects missing IDs and duplicate classifications. An exclusion proposal is
listed with its reason, remains in the denominator, and has no native credit.
The seven platform/build skips and the default-locale skip retain their original
reference reasons even where UTF-8 mode also provides native evidence.

## Evidence per occurrence

[evidence-linux.json](evidence-linux.json) is the versioned [shared evidence
contract](../../NATIVE_EVIDENCE.md): 97 distinct occurrences at each of O0 and O3.
This baseline was executed with Imre compiler/runtime source `7630556901c9936c9ca112f7fb2f4e52f3706fd3`. The pinned suite has no companion module. The two shorter `native-O*.json`
files retain their original records and are not additional test occurrences.

The versioned gate loads the original suite and runs its selected methods
under their CPython fixtures. For every remaining non-skipped method it either
tries to compile the complete definition or records the original decorator
that requires an adapter. An unreviewed exception proposal remains a blocker.
The actual native CLI failure for `test_orig_argv` is recorded separately.
The unchanged locale-dependent method can pass under `-X utf8` even though
the reference skipped it; the contract records that as a reference disagreement.

Pass `--evidence /tmp/sys-evidence-O$opt.json` to the command above for each
optimization level. Combine and validate the two files with:

```sh
jq -s . /tmp/sys-evidence-O0.json /tmp/sys-evidence-O3.json > /tmp/sys-evidence.json
python3 benchmarks/cpython_stdlib/suite_evidence.py benchmarks/cpython_stdlib/full/sys/reference-linux.json /tmp/sys-evidence.json
```
