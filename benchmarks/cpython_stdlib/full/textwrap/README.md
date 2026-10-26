# textwrap

The [manifest](UPSTREAM.json) pins CPython 3.14.5's complete module and test
file. All 68 original methods pass on Linux CPython; none is skipped.

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

The [per-case ledger](native-linux.json) accounts for all 68 methods.
Whole-file execution still stops at the original `BaseTestCase.check_wrap`
helper, which accepts and forwards `**kwargs`.

```sh
python3 benchmarks/cpython_stdlib/full/textwrap/run.py \
  --imre target/release/imre --runtime target/release/libimre_runtime.a \
  --clang /usr/bin/clang-18 --cpython-source /path/to/cpython-3.14.5 \
  --result /tmp/textwrap-native.json
```
