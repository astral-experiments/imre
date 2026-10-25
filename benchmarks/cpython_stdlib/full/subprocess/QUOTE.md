# Command-line quoting

At O0 and O3 the two original `test_list2cmdline` occurrences ran their complete method, eight equality assertions and command-line quoting on Imre. CPython ran their module, class and instance fixtures, including the NoPoll setup and restoration. These are two native bodies with host fixtures in a denominator of 353. Formatting a command line launches no child.

`quote_originals.py` executes the original method with the shared suite runner, then links the same method with a C observer around the actual native quoting and joining functions. It observes 23 argument quotes and eight returned command lines. On eight separate runs per method, after the real join returns, it substitutes a wrong value at one selected join. That exact original assertion fails and no later argument is consumed.

With `suite=benchmarks/cpython_stdlib/full/subprocess` and the pinned CPython 3.14.5 interpreter:

```sh
for level in 0 3; do
    "$python" -E -B "$suite/quote_originals.py" --imre "$imre" --runtime "$runtime" \
        --clang "$clang" --opt-level "$level" --result "quote-$level.json"
done
git show "origin/main:$suite/evidence-linux.json" > previous-linux.json
"$python" -E -B "$suite/quote_evidence.py" --previous previous-linux.json \
    --quotes "$suite/quote-linux.json" --result evidence-linux.json
```

`quote-linux.json` pairs the reports with the exact product revision and tree. Each report identifies the original method, both linked programs, the observer and toolchain by hash. The generator verifies the arguments, assertions and host fixture labels before updating these two rows, and checks that every other occurrence is unchanged.

The product differential also covers path callbacks, mixed string/bytes/path lists, iteration order, NUL and trailing backslashes. On Linux, Imre's scalar strings cannot represent surrogateescaped bytes: pinned CPython returns a single U+DCFF for `list2cmdline([b'\xff'])`; Imre raises `UnicodeDecodeError`. Neither original method uses such an argument.
