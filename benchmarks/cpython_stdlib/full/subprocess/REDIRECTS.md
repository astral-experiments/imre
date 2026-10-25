# Original file redirection fixtures

[redirect_fixtures.py](redirect_fixtures.py) ran all 14 occurrences of the file and
integer-descriptor tests with the pinned CPython module, class and instance fixtures.
The original host body, assertions, temporary file and cleanup remain in CPython.
A separate compiled Imre parent accepts the original argument list, launches that
CPython data producer or consumer, and services the original wait and returncode calls.

For an integer, the parent inherits the same descriptor number. For a file object,
the native parent uses the OS implementation of `dup2` to attach a real native file
object to a duplicate of the original open file description. When the original passes
one object twice, the native call receives the same native object twice. A host check
immediately after native wait observed the original file's offset advance from zero
to 4 (stdin), 6 (stdout), 10 (stderr), or 11 (combined), before the original assertion
read or sought on that file.

At both O0 and O3 on Linux, all 14 host-driven native operations passed. A trace of
each native parent observed no selector calls, including in the seven NoPoll cases.
A separate timed communicate positive control made two poll calls at each level.
[redirects-linux.json](redirects-linux.json) lists the individual actor source/binary
and toolchain hashes. Those measurements used subprocess `1012f2f` and OS descriptor
revision `af3391e` (subsequently merged in #506). The six integer cases also passed
on subprocess `96b9434` without the OS integration. These are existing occurrences
of the 353-test suite, not extra tests.

Run with the pinned `python` in a Linux environment with strace:

```sh
python benchmarks/cpython_stdlib/full/subprocess/redirect_fixtures.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a --clang /path/to/clang \
    --opt-level 0 --kind all --result /tmp/redirects-0.json
```

Repeat with `--opt-level 3`. Use `--kind numbers` or `--kind files` to run one
representation separately.

The [repeat with the merged launch-error and file-descriptor code](redirects-merged-linux.json)
records the same offsets at both levels. Its tested tree is
`5619286a93eced789acfa99b1930ec12a98c585f`; the compiler and runtime sources
are identical to the green #651 head. The complete reports include these
14 occurrences as host-driven native operations. To reproduce that update:

```sh
"$CPYTHON" redirect_evidence.py --previous /tmp/previous-evidence.json \
    --result evidence-linux.json
```
