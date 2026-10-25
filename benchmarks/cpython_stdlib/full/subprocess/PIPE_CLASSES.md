# Popen stream classes

Both original process fixtures run `test_io_buffered_by_default` and
`test_io_unbuffered_works`. CPython supplies the module fixtures, unchanged
method and assertions. Each stream proxy asks the compiled parent to evaluate
`isinstance` against all four IO base classes; the proxy translates the returned
membership bits for the host assertion. All three streams retain their class
after close. A Linux syscall trace identifies the native executable, the real
fixture child, the three descriptors the parent closes, and one successful reap.
No native selector is invoked by these methods. The complete method compiler
attempt and its diagnostic are retained separately.

To reproduce the entries in [pipe-classes-linux.json](pipe-classes-linux.json),
run with the pinned interpreter from the module directory, with `strace` available:

```sh
for level in 0 3; do
    "$CPYTHON" pipe_class_originals.py --imre "$IMRE" --runtime "$RUNTIME" \
        --clang "$CLANG" --opt-level "$level" --result "/tmp/pipe-class-$level.json"
done
"$CPYTHON" pipe_classes_evidence.py --previous /tmp/previous-evidence.json \
    --pipe-classes pipe-classes-linux.json --result evidence-linux.json
```
