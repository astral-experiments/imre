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

## Native execution

[native.py](native.py) copies complete undecorated methods from the verified
source and compiles each with an assertion adapter. The adapter checks equality
and returns an assertion count from the executable. Both the method and every
operation it invokes run in Imre; CPython does not supply iterator results.

The [O0](native-O0.json) and [O3](native-O3.json) standalone controls pass thirteen
complete bodies: four repeat methods, six examples of chain/chain.from_iterable/compress/count/cycle/islice,
the twenty-million-item backward tee deletion regression, and two builtin
filter/zip examples. Their native fixture records assertion counts.
The [per-occurrence reports](COVERAGE.md) separately run the original unittest
fixtures and identify where each body, assertion, product operation and fixture ran.
The larger repeat test also needs complex numbers and callable exception
assertions; the repeat GC case creates a cycle between a list and its iterator.
Neither is implied by the four passing repeat methods.

```sh
for opt in 0 3; do
    python3 benchmarks/cpython_stdlib/full/itertools/native.py --imre "$IMRE" --runtime "$RUNTIME" \
        --clang "$CLANG" --opt-level "$opt" --result "/tmp/itertools-O$opt.json"
done
```

[repeat.py](repeat.py) separately compares interleaved pulls from the same
cursor, a live mutable element, repeated exhaustion, eager evaluation of the
default in `next()`, and constructor overflow with CPython. It adds no suite ID.


[sources.py](sources.py) additionally compares partial consumption through two
aliases, a stop before the start, live list changes during cycle's first pass,
mutation during islice argument evaluation, release after a dictionary size
error, and the timing of float step conversion. These differential checks are
outside the denominator. The various-iterator-argument methods still require
Python-defined iterator protocols, including sources that raise mid-pull.


[tee.py](tee.py) compares independent positions, forks of a partly consumed
fork, a discarded lagging fork, source mutation, and an empty result that must
not obtain an iterator. The larger `test_tee` body also requires weak references,
dynamic types and arbitrary Python source iterators; it is not counted.

[batched.py](batched.py) compares constructor argument evaluation, partial
consumption, strict and non-strict tails, and sticky exhaustion after input
errors. It interleaves a batch cursor with its source CSV reader or text file,
including a CSV parse error and closing a file before or after acquisition.
With a disposable filename argument it appends behind a live text reader:
a non-strict short batch resumes, but an empty pull or strict failure stops it.
On Linux, [batched_allocation.py](batched_allocation.py) fails the
first native tuple allocation and compares it with a test-enabled pinned
CPython using `_testcapi.set_nomemory`. Both keep the source at its first item
and allow a later pull on the same batch cursor. Neither control adds a test ID.

[chain.py](chain.py) compares a live outer list with starred arguments captured
at construction. Both see later changes to an inner list. It also covers a
shared inner cursor, delayed dictionary acquisition, and CSV parse errors:
an inner reader can resume at its next row; an outer reader stops the chain
and leaves that row for its other consumer. The separate native stack test
advances and releases 16,384 nested chains while preserving an aliased leaf.

[compress.py](compress.py) compares data and selector positions, propagation
of selector errors, argument acquisition order, and truth of builtin values.

The selector regression also separates data errors from selector errors: only
the latter discard a data item. Empty iterator selectors test true without a
pull. The runtime controls pause an outer pull while a second consumer reads
the same cursor, and traverse 16,384 selector adapters on a 256 KiB stack.

[accumulate.py](accumulate.py) exercises lazy seed and callback evaluation,
aliasing a live input, exceptions raised by a callback or by its source,
and resuming after a callback raises `StopIteration`. It also checks how
chain, batched, cycle, tee and islice consume that same transient exception.
These supplemental operations add no test ID. The unchanged
`TestExamples.test_accumulate` is counted separately in the
[per-occurrence reports](COVERAGE.md).
