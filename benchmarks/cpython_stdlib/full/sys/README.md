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

- `test_original_displayhook`: CPython runs the original body, assertions and capture fixture; ten operations execute in one native process, including stdout deletion and restoration.
- `test_sys_exception_no_exception`: the complete method queries native handler state and compares the result to `None`. A separate program checks object identity across nested handlers, calls and handler exit.
- `test_setrecursionlimit`: the complete unchanged method runs in Imre. It sets the limit to 10,005, checks the new value and both argument errors, then restores the original limit in its `finally` block. CPython runs the original `SysModuleTest` teardown.
- `test_getrecursionlimit`: the complete body and assertions run in Imre. The native getter returns an integer greater than one and rejects the extra argument. The sys fixture checks the real class operand and requires two swapped-class assertions to fail while shared `assertIsInstance` is unavailable. CPython runs the original teardown. Separate native controls compare frame depth, unwinding and integer conversion against stock. `test_recursionlimit_recovery` still needs recursive source calls; `test_setrecursionlimit_to_depth` needs real frame depth, including `test.support.get_recursion_depth`'s `_getframe`/`f_back` fallback.
- `test_43581`: the complete method reads both stream attributes and executes the shared native unittest string equality assertion. CPython runs the original fixture.
- The [filesystem encoding probe](filesystem_encoding.py) runs the [same program](filesystem_encoding_case.py) with both interpreters. Its [Linux results](filesystem-encoding-linux.json) cover 18 native executable launches and five `imre -c` launches at each optimization level. The C, POSIX, C.UTF-8 and generated Latin-1 locales distinguish filename encoding from `PYTHONIOENCODING`; `-I` and `-X utf8` exercise the actual command boundary. The program checks the three getter values, repeated object identity, interning, codec names and the order and text of argument errors. These process controls add no occurrences. The native registry still rejects unsupported locale codesets.
- `test_intern`: the gate tries the complete native method before running the original parent with one child for string handles. Each call observes that same table, and the parent compares the returned identities. For the local class `S(str)`, the fixture compiles the original class and its `__hash__` in a separate child. At O0 and O3, four argument errors and five string identity or status checks reached Imre. The compiler rejected the original subclass before the tenth call. Both occurrences remain blocked.
- `test_getdefaultencoding`: the complete method runs both the callable `assertRaises` and the result type assertion in Imre. CPython runs the original fixture and teardown. When the shared TestCase lacks `assertIsInstance`, the [encoding fixture](encoding_methods.py) checks the requested native class and compares its failure paths with CPython.
- `test_getfilesystemencoding`: the unchanged method and its unchanged `check_fsencoding` helper run in the native binary. The helper calls native `codecs.lookup`, and its assertions run there too. The pinned CPython suite retains its original instance lifecycle and `SysModuleTest.tearDown`.
- `test_module_names`: the complete unchanged method checks the concrete frozen set and iterates all 297 strings in Imre. The original CPython teardown runs on the host. The shared native TestCase does not yet supply `assertIsInstance`; the [local assertion](module_names_case.py) checks the two actual class operands and must reject both swapped operands at each optimization level.
- `test_jit_is_available` and `test_jit_is_enabled`: Imre answers the parent capability query and executes both original child assertions under `PYTHON_JIT=0` and `=1`. CPython runs the parent, fixtures and child-status assertions. The fixture builds the original child source directly; its automatic `-X faulthandler` option is outside this check.
- `test_thread_info`: the original CPython body and assertions read the native thread record, length, name, lock and target platform. A separate operation check verifies the singleton, slice, field names and library version. On glibc the target reads the pthread version through `confstr`; separate controls cover a changed version, an unavailable version and a truncated result, and require only one query per process. Imre reports `lock=None` because it exposes neither of CPython’s named thread-lock backends. The full body still needs native `TestCase.assertIn` and membership over string/None tuple elements.
- `test_implementation`: the original parent reads the five PEP 421 fields, the version components and the version slice in Imre. The 30 native operations include the implementation name used by the original CPython-only branch guard. The pinned CPython parent owns the assertions and fixture. Imre reports its own release, no bytecode cache tag, and no isolated interpreters.
- `test_ioencoding_nonascii`: the canonical reference and native gate both skip under the unchanged locale decorator. See the separate UTF-8-mode run below.
- `test_c_locale_surrogateescape` and `test_posix_locale_surrogateescape`: the [locale observer](locale_stream_cases.py) runs each original method and its two helpers against both executables. Stock CPython passes six children for each locale. At O0 and O3 the first native child fails to compile `getattr(sys, name)` at line 3, column 19; no stream property is read. The reports keep that child, the first failed host assertion and the whole-method compile attempt. The original places `-I` after the `-c` source in the first child, so it is preserved as a script argument. These two observations were measured against source `0caa770230d5f5682f166fdc75899dd607b5fde3` (tree `f55d657587228e6c94def6565dfacc0ac0c3e07b`).
- `test_ioencoding`: the unchanged CPython method and fixture launch all five native children at O0 and O3. CP424 writes `4a 25`, ASCII replacement writes `3f 0a`, and both strict ASCII settings raise `UnicodeEncodeError`. The fifth exits without writing because Imre cannot construct U+DCBD. The method remains a failure. The [stdout comparison](stdout-encoding-linux.json) records each child independently; the [CP424 registry comparison](codec-cp424-linux.json) covers all 256 byte values.

The existing [stream and exit smoke runner](../../native_sys.py) exercises four
`test_subprocess` IDs and additional native program fixtures, including the
[standard-stream buffering checks](STREAM_BUFFERING.md). Those are outside this
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
The [Linux CI run](https://github.com/astral-sh/imre/actions/runs/40173735073) executed source `35158607886bce3fec10dd8e965d891e803e1088` (tree `f80a4bcd512cdb24ff2f279f59adb6d2075bef8a`). Its `sys-evidence-linux` artifact contains both optimization reports, the legacy outcomes, and the tested commit and tree. The original displayhook method performs its ten operations inside one native process ([builtin coverage](BUILTINS.md)). The six native reads in `test_thread_info` were measured at `f369a158456a9847f3f827d5f10913194dede25f`; CPython ran the original body and assertions. The pinned suite has no companion module; `native-O*.json` describes the same occurrences.

The [combined encoding run](https://github.com/astral-sh/imre/actions/runs/40223343057/job/121259456818) ran both original encoding methods and their assertions natively. CPython supplied their fixtures.
The blocked `test_intern` rows retain the [intern run](https://github.com/astral-sh/imre/actions/runs/40179084540) on source `d583f2dd2cc78051c053d93d9765135dafaeb049`.
The recursion-limit getter and setter also ran as complete native bodies under their original CPython fixtures. [Earlier Linux run](https://github.com/astral-sh/imre/actions/runs/40184296027)

The 30 native operations in `test_implementation` were measured at `dc0ea48199b4afbef27eeeca5aeb4f85224acabe`. The metadata record exposes five PEP 421 fields with distinct native types. Arbitrary namespace attributes and the Python dictionary interface are not measured here.

At each level the two JIT capability methods add two mixed results. `test_jit_is_active` made one native availability query in the parent; its first original child stopped at line 2 on `import _testcapi` before any `is_active` call or child assertion. The second child was not reached. Its individual private-API proposal remains unapproved.

The original `test_executable` runs its own assertions around a native process launched with an altered program name. Its stock reference and native child are recorded separately in the occurrence report.

The versioned gate loads the original suite and runs its selected methods
under their CPython fixtures. For every remaining non-skipped method it either
tries to compile the complete definition or records the original decorator
that requires an adapter. An unreviewed exception proposal remains a blocker.

Five of the six `ActiveExceptionTests` methods, `test_attributes` and `test_sys_version_info_no_instantiation` compile but stop at the generated top-level `import unittest`. Each record keeps the native child's exit status and traceback. CPython ran the host fixture. The body, assertions and sys operation are unobserved. The sixth method, `test_sys_exception_no_exception`, executes its body natively under the host fixture. The gate also runs two deliberate native body failures under the same TestCase driver, including one that raises the identical `ModuleNotFoundError`. [Original Linux failures](https://github.com/viarius-experiments/imre/actions/runs/40451912852/job/122016395024)

The unchanged `test_exit` runs under its original CPython fixture and assertions. Six caught calls and five original child programs complete in Imre, including `2**128` and a pending stderr write. The next child contains a lone U+DCFF. On `3df6330a` it compiled and wrote U+FFFD (`ef bf bd`) to stderr. CPython expected the six ASCII bytes for `\udcff`. The compiler now rejects that unsupported literal before the child runs. The later Latin-1 child, run independently, still returns status 1, empty stdout and stderr bytes `68 e9 0a` at both levels. [Original child probes](stderr-encoding-linux.json)

Pass `--evidence /tmp/sys-evidence-O$opt.json` to the command above for each
optimization level. Combine and validate the two files with:

```sh
jq -s . /tmp/sys-evidence-O0.json /tmp/sys-evidence-O3.json > /tmp/sys-evidence.json
python3 benchmarks/cpython_stdlib/suite_evidence.py benchmarks/cpython_stdlib/full/sys/reference-linux.json /tmp/sys-evidence.json
```

CI compares the checked-in and measured outcome, classification, reference, fixture variant and execution roles for each test ID, occurrence and optimization level. Both full reports are validated first. Raw diagnostics, including checkout paths and process IDs, remain in the evidence artifact.

A manual run of the CI workflow can verify a prospective merge commit on Linux.
Publish the commit to this repository, then supply its full SHA as the
`sys_source` input. The job builds that commit and runs the same sys commands
as pull request CI. Its `sys-evidence-linux` artifact records the tested Git
commit and tree alongside both reports. Pull request checks run separately.

## Native stack introspection

`test_getframemodulename` remains blocked at native unittest equality between a
string and an optional string; the later loop needs real frame objects and their
`f_back` links. A [separate program](frame_names.py) and its [imported functions](frame_names_owner.py)
compare native and stock module names for nested calls, index conversion and exception
unwinding. They also alternate limits 2 and 3 across a named call and check the
caller's module identity after each return or rejected entry. On [source 92c7e5e](https://github.com/astral-sh/imre/commit/92c7e5e5d447a5e02d6ab879c8ff48849da48696),
all 20 output lines matched pinned CPython at O0 and O3. Each blocked occurrence
records the separate control and zero operations in the unchanged method.

## Two process-boundary cases

`test.test_sys.SysModuleTest.test_ioencoding_nonascii` compares the spelling of two codec names before it launches a child. The [locale audit](locale-audit-linux.json) runs the unchanged method against both subjects in each host configuration. Only the canonical row enters the 97-occurrence report.

| CPython host (LC_CTYPE=C.UTF-8) | Filesystem | Preferred | Original decorator and child |
| --- | --- | --- | --- |
| Default, UTF-8 mode 0 (reference) | `utf-8` | `UTF-8` | Skipped: “requires FS encoding to match locale”; no child |
| `-X utf8`, mode 1 | `utf-8` | `utf-8` | Stock CPython and Imre both write hex `c3a60a` at O0/O3 |

In the second row the original parent sets `PYTHONIOENCODING=""` for the child; it does not pass its own `-X utf8` to the child. CPython executes the original parent, comparison to `os.fsencode`, and fixture. Imre compiles only the child source. To reproduce either setting, use the gate command above with `--locale-audit /tmp/locale.json` in place of `--result`; add `-X utf8` to the host for the second row.

`test.test_sys.SysModuleTest.test_orig_argv` launches `<executable> -I -X utf8 -c CODE arg` and asserts two printed lists. In [the prior audit](https://github.com/astral-sh/imre/pull/715), option dispatch failed before either sys read. Imre now compiles and runs those exact command bytes and returns 0 with empty stderr. At O0/O3 the native child reports `['-c', 'arg']` and the entire original invocation. The original parent comparison, subprocess fixture, and teardown run on CPython. The [per-occurrence report](evidence-linux.json) records both subjects, the child hash, the two native reads, and independent file and `imre run` controls. The stock test ID still occurs once per level.

[Standard input decoding](STDIN.md) covers native stdin, its startup metadata
and retry behavior. These controls add no upstream IDs.
