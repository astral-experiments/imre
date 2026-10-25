# Native execution of the collections doctests

Run from this directory with the pinned test-enabled CPython 3.14.5:

```sh
for level in 0 3; do
    "$PYTHON" -B doctests.py --imre "$IMRE" --runtime "$RUNTIME" --clang "$CLANG" \
        --opt-level "$level" --result "doctests-O$level.json"
done
```

The runner attempts **all 16 doctest IDs** in the full collections inventory,
including the deque library reference. Each Imre executable runs one complete
doctest with a shared namespace for its prompts. The harness supplies the
interactive display hook and compares each prompt's output with the original
expectation. It checks that hook on the reference build before compiling and
returns a failure status if any native case is blocked or fails.

At both [O0](doctests-O0.json) and [O3](doctests-O3.json), the native programs
execute 12 complete doctests: Counter construction, update, subtract,
`most_common`, addition, subtraction, union, intersection and their four
in-place forms. Their output checks run on the host. Four compile blockers
remain: the Counter class examples, `elements`, the namedtuple examples and
the deque library reference.
These 16 IDs are already included in the 659-case denominator.
