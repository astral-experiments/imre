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

To run the encoding cases against the compiler and runtime under test:

```sh
for opt in 0 3; do
    "$CPYTHON" -X utf8 benchmarks/cpython_stdlib/full/sys/run.py \
        --imre "$IMRE" --runtime "$RUNTIME" --clang "$CLANG" \
        --opt-level "$opt" --result "/tmp/sys-O$opt.json"
done
```

- `test_43581`: the complete method, both stream attributes, equality and assertion execute in the native binary.
- `test_getdefaultencoding` and `test_getfilesystemencoding`: all `sys` operations execute in the native driver. The original unittest assertions, codec lookup and transport run on CPython.
- `test_ioencoding_nonascii`: the original subprocess source executes as a native program. CPython owns the fixture, process pipes and expected filesystem bytes. The default Linux reference skips this test because the locale and filesystem codec names differ in case; `-X utf8` satisfies its unchanged decorator.

The existing [stream and exit smoke runner](../../native_sys.py) exercises four
`test_subprocess` IDs and additional native program fixtures. Those are outside this
suite's denominator. A CPython-only result or an operation probe is not a native
pass for a complete routine.
