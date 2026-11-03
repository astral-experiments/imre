# Forward references in annotationlib and typing

Stock CPython ran each method from the complete upstream module and again
with the module root used for the ordinary native builds. The native roots
contain one original method under its original class and module. The
annotationlib root includes the upstream `support.EqualToForwardRef` helper.
The typing root includes the upstream `BaseTestCase`.

[Source and binary hashes, suite ordinals and counts](results.json) ·
[Registration, unittest callback and exit for each process](callbacks.tsv)

The annotationlib control calls the real evaluator with locals
`{'a': 1, 'b': 2}`, checks that it returned the integer 3 and substitutes 4.
The typing original checks four class annotations. Its control calls the real
`get_type_hints` for the class with the string `'ClassVar[int]'`, verifies
the result and substitutes `ClassVar[str]`. In each run the upstream assertion
rejects the substitution. These control runs contribute zero parents.

## Reproduce on Linux

Use test-enabled CPython at the revision in `results.json`. From this
directory, set `STOCK` to that interpreter, `IMRE` and `RUNTIME` to the
compiler and runtime built from the recorded Imre revision, and `CLANG` to
the Clang driver. The stage builder checks each generated file against its
measured digest.

```sh
set -eu
work=$(mktemp -d)
python3 stage.py "$work"
for family in annotationlib typing; do
    root="$work/$family"
    for mode in pass wrong; do
        expected=0
        if [ "$mode" = wrong ]; then expected=1; fi
        code=0
        "$STOCK" -I -B -c 'import runpy, sys; runpy.run_path(sys.argv.pop(1), run_name="__main__")' \
            "$root/entry.py" "$mode" || code=$?
        test "$code" = "$expected"
        code=0
        "$STOCK" -B "$root/entry.py" "$mode" || code=$?
        test "$code" = "$expected"
    done
    for level in 0 3; do
        elf="$work/$family-O$level"
        "$IMRE" build "$root/entry.py" --module-root "$root" --no-cache \
            --opt-level "$level" --runtime "$RUNTIME" --clang "$CLANG" -o "$elf"
        PATH=/nonexistent "$elf" pass
        code=0
        PATH=/nonexistent "$elf" wrong || code=$?
        test "$code" = 1
    done
done
```

Each invocation should register and run its canonical ID exactly once.
Compare the terminal callback, assertion and exit code with `callbacks.tsv`.
