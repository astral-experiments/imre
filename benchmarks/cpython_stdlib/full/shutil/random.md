# The random dependency in the zero-copy fixture

CPython 3.14.5's `test_shutil.write_test_file` builds a chunk by calling `random.choice(string.ascii_letters)`. The shared random implementation seeds MT19937 from OS entropy or an explicit integer, string or bytes value. Its module-level `random`, `getrandbits`, `randint`, `randrange` and `choice` share that state. Choice supports native strings, bytes, lists and direct range expressions. This covers the dependency, not the entire random module.

The [native comparison](random-consumer-linux.json) used the [Linux check](https://github.com/astral-sh/imre/actions/runs/40192071855) of `e8bfaa1`. Its verification branch differs only in the CI workflow. The exact upstream expression was taken from the pinned test blob. All 10,000 seeded choices matched at both O0 and O3. Additional controls exercise generator twists, integer and text seeds, error types and messages, single evaluation of the input, and the range selection used by functools. Each child ran in a separate temporary directory.

The complete original file-writing helper has a separate result. CPython wrote a 4,096-byte file; native compilation stops at the unannotated nested `chunks` function. The [four whole-suite startup modes](random-startup-linux.json) reached line 18, `shutil.make_archive`, and executed zero tests. These controls add no native pass to the 223 original shutil occurrences.

Reproduce with test-enabled CPython 3.14.5:

```sh
/path/to/python -B benchmarks/cpython_stdlib/full/shutil/random_consumer.py \
    --imre /path/to/imre --runtime /path/to/libimre_runtime.a \
    --clang /path/to/clang --result /tmp/shutil-random.json
```
