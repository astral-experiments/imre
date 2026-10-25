# CPython 3.14.5 itertools

The unchanged [test_itertools.py](upstream/Lib/test/test_itertools.py) and
[license](upstream/LICENSE) match the Git blobs in [UPSTREAM.json](UPSTREAM.json).
This is one test module, with no package submodules. Its `load_tests` hook also
loads the `itertools.batched` doctest from the reference module's docstring.

The [Linux reference run](reference-linux.json) lists all **137 IDs** (136
unittest methods and one doctest). At default resources 132 pass and five skip.
Running the same suite with `-u cpu` also passes
`RegressionTests.test_long_chain_of_empty_iterables`: **133 pass, four skip**.
The remaining four are the `@support.bigaddrspacetest` methods for product,
combinations, combinations with replacement, and permutations. The decorator
reports insufficient address space on this 64-bit platform. All remain in the
denominator. No Imre result is inferred from the reference run.

From the repository root, with the test-enabled pinned interpreter:

```sh
"$CPYTHON" -B benchmarks/cpython_stdlib/suite_inventory.py \
    benchmarks/cpython_stdlib/full/itertools/UPSTREAM.json --result /tmp/itertools-reference.json
"$CPYTHON" -B -m test --single-process --randseed 1 -u cpu --junit-xml /tmp/itertools-cpu.xml test_itertools
```
