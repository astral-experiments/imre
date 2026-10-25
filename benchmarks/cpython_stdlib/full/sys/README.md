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
env -u PYTHONUTF8 -u PYTHONIOENCODING LC_ALL=C.UTF-8 LANG=C.UTF-8 \
    "$CPYTHON" -B -m test --single-process --randseed 1 --junit-xml /tmp/sys.xml test_sys
```

To run the selected full methods against the compiler and runtime under test, use
the [shared suite harness](../../SUITE_RUNNER.md). Module, class and instance
fixture hooks run in the pinned CPython host. Each result records where the
original body, its assertions and the tested operations execute.

```sh
for opt in 0 3; do
    env -u PYTHONUTF8 -u PYTHONIOENCODING LC_ALL=C.UTF-8 LANG=C.UTF-8 \
        "$CPYTHON" -B benchmarks/cpython_stdlib/full/sys/run.py \
        --imre "$IMRE" --runtime "$RUNTIME" --clang "$CLANG" \
        --opt-level "$opt" --result "/tmp/sys-O$opt.json"
done
python3 benchmarks/cpython_stdlib/full/sys/coverage.py --result /tmp/sys-coverage.json
```

- `test_sys_exception_no_exception`: the complete method queries native handler state and compares the result to `None`. A separate program checks object identity across nested handlers, calls and handler exit.
- `test_43581`: the complete method, both stream attributes, equality and assertion execute in the native binary.
- `test_intern`: the gate tries the complete native method before running the original parent with one child for string handles. Each call observes that same table, and the parent compares the returned identities. For the local class `S(str)`, the fixture compiles the original class and its `__hash__` in a separate child. At O0 and O3, four argument errors and five string identity or status checks reached Imre. The compiler rejected the original subclass before the tenth call. Both occurrences remain blocked.
- `test_getdefaultencoding` and `test_getfilesystemencoding`: all `sys` operations execute in the native driver. The original unittest assertions, codec lookup and transport run on CPython.
- `test_jit_is_available` and `test_jit_is_enabled`: Imre answers the parent capability query and executes both original child assertions under `PYTHON_JIT=0` and `=1`. CPython runs the parent, fixtures and child-status assertions. The fixture builds the original child source directly; its automatic `-X faulthandler` option is outside this check.
- `test_ioencoding_nonascii`: the canonical reference and native gate both skip under the unchanged locale decorator. See the separate UTF-8-mode run below.

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
The [Linux CI run](https://github.com/astral-sh/imre/actions/runs/40179084540) executed source `d583f2dd2cc78051c053d93d9765135dafaeb049` (tree `6c1fd240e3769ce746d2acfeccf43c8f284508ad`). Its `sys-evidence-linux` artifact contains both optimization reports, the legacy outcomes, and the tested commit and tree. The checked-in reports match those records. The pinned suite has no companion module; `native-O*.json` describes the same occurrences.

At each level the two JIT capability methods add two mixed results. `test_jit_is_active` made one native availability query in the parent; its first original child stopped at line 2 on `import _testcapi` before any `is_active` call or child assertion. The second child was not reached. Its individual private-API proposal remains unapproved.

The versioned gate loads the original suite and runs its selected methods
under their CPython fixtures. For every remaining non-skipped method it either
tries to compile the complete definition or records the original decorator
that requires an adapter. An unreviewed exception proposal remains a blocker.
The unchanged `test_exit` runs under its original CPython fixture and assertions: six caught calls and five original child programs complete in Imre, including `2**128` and a pending stderr write. The next unchanged child source contains a lone U+DCFF. Its native compile failure blocks the method before the final Latin-1 check; no partial method receives pass credit.

Pass `--evidence /tmp/sys-evidence-O$opt.json` to the command above for each
optimization level. Combine and validate the two files with:

```sh
jq -s . /tmp/sys-evidence-O0.json /tmp/sys-evidence-O3.json > /tmp/sys-evidence.json
python3 benchmarks/cpython_stdlib/suite_evidence.py benchmarks/cpython_stdlib/full/sys/reference-linux.json /tmp/sys-evidence.json
```


## Two process-boundary cases

`test.test_sys.SysModuleTest.test_ioencoding_nonascii` compares the spelling of two codec names before it launches a child. The [locale audit](locale-audit-linux.json) runs the unchanged method against both subjects in each host configuration. Only the canonical row enters the 97-occurrence report.

| CPython host (LC_CTYPE=C.UTF-8) | Filesystem | Preferred | Original decorator and child |
| --- | --- | --- | --- |
| Default, UTF-8 mode 0 (reference) | `utf-8` | `UTF-8` | Skipped: “requires FS encoding to match locale”; no child |
| `-X utf8`, mode 1 | `utf-8` | `utf-8` | Stock CPython and Imre both write hex `c3a60a` at O0/O3 |

In the second row the original parent sets `PYTHONIOENCODING=""` for the child; it does not pass its own `-X utf8` to the child. CPython executes the original parent, comparison to `os.fsencode`, and fixture. Imre compiles only the child source. To reproduce either setting, use the gate command above with `--locale-audit /tmp/locale.json` in place of `--result`; add `-X utf8` to the host for the second row.

`test.test_sys.SysModuleTest.test_orig_argv` launches `<executable> -I -X utf8 -c CODE arg` and asserts two printed lists. In [the prior audit](https://github.com/astral-sh/imre/pull/715), option dispatch failed before either sys read. Imre now compiles and runs those exact command bytes and returns 0 with empty stderr. At O0/O3 the native child reports `['-c', 'arg']` and the entire original invocation. The original parent comparison, subprocess fixture, and teardown run on CPython. The [per-occurrence report](evidence-linux.json) records both subjects, the child hash, the two native reads, and independent file and `imre run` controls. The stock test ID still occurs once per level.
