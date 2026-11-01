# textwrap

The [manifest](UPSTREAM.json) pins CPython 3.14.5's complete module and test
file. All 68 original methods pass on Linux CPython. On Imre, importing the eight
original case classes into `unittest.main()` runs all 68 cases in loader order at
O0 and O3. Both native runs passed without skips. The unmodified test file also runs all 68
cases through its own `unittest.main()` entry. See the [per-occurrence results](evidence-linux.json).

## Baseline replay

The [baseline ledger](native-linux.json) records the following results from
the subset and replay runner.

| Original cases at O0 and O3 | Count | What ran |
| --- | ---: | --- |
| Fully native | 15 | Fourteen original dedent and indent cases plus the shortening placeholder boundary, with original class fixtures, assertions, and the native unittest entry |
| Host-driven native operations | 53 | Original CPython bodies and assertions consume native functions' return values or exceptions |

The [runner](run.py) extracts complete original methods without editing their
assertions. For the four methods that bind an unannotated local lambda, it
compiles the original predicate expression into the native call. For the
dedent type-error method, CPython runs the original regex assertions against
both messages produced by Imre. A separate negative control substitutes
incorrect results and requires all fourteen indentation method bodies to fail.
The original shortening test additionally rejects an implementation that omits
the required placeholder error. The [wrapper replay](wrapper_cases.py) includes
constructor arguments, per-instance mutation, and successive calls on the same object.
The [edge cases](edge_cases.py) check predicate call order, Unicode line
endings, and exceptions during argument evaluation and iteration.

```sh
python3 benchmarks/cpython_stdlib/full/textwrap/run.py \
  --imre target/release/imre --runtime target/release/libimre_runtime.a \
  --clang /usr/bin/clang-18 --cpython-source /path/to/cpython-3.14.5 \
  --result /tmp/textwrap-native.json
```

## Running the unmodified module

Run the [module runner](module_suite.py) with CPython 3.14.5 on Linux. It stages
and verifies the pinned test sources, gets the ordered IDs from CPython's loader
and checks Imre's verbose results against those IDs at both O0 and O3.

```sh
python3.14 -B benchmarks/cpython_stdlib/full/textwrap/module_suite.py \
  --imre target/release/imre --runtime target/release/libimre_runtime.a \
  --clang /usr/bin/clang-18
```

The default entry calls `unittest.defaultTestLoader.loadTestsFromModule`
and propagates an unsuccessful result. In this standalone checkout the compiled
driver exits before discovery with `ModuleNotFoundError: No module named 'unittest'`.
Use `--classes` for the
separate `unittest.main()` entry that imports all original case classes, or
`--classes DedentTestCase IndentTestCase` for just those complete classes.
The runner returns a failing status for compiler errors, unsuccessful native
runs, or missing or out-of-order results. Add `--result PATH` to write paired
evidence after both selected runs pass. Unselected occurrences retain their
place in the denominator.
