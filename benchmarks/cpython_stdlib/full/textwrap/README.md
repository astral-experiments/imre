# textwrap

The [manifest](UPSTREAM.json) pins CPython 3.14.5's complete module and test
file. All 68 original methods pass on Linux CPython; none is skipped.

| Original cases at O0 and O3 | Count | What ran |
| --- | ---: | --- |
| Fully native | 7 | Original dedent bodies, helper, assertions, and native unittest entry point |
| Native body, adapted fixture | 7 | Original indent bodies and assertions; a native property returns the original CPython class tuple |
| Host-driven native operations | 5 | Original CPython test bodies and assertions consume the native functions' return values or exceptions |
| Reference only | 49 | Wrapping, filling, shortening, and other cases without native evidence |

The [runner](run.py) extracts complete original methods without editing their
assertions. For the four methods that bind an unannotated local lambda, it
compiles the original predicate expression into the native call. For the
dedent type-error method, CPython runs the original regex assertions against
both messages produced by Imre. A separate negative control substitutes
incorrect results and requires all fourteen native method bodies to fail.
The [edge cases](edge_cases.py) check predicate call order, Unicode line
endings, and exceptions during argument evaluation and iteration.

The [per-case ledger](native-linux.json) accounts for all 68 methods.
The original full suite entry still stops at its import of `TextWrapper`.

```sh
python3 benchmarks/cpython_stdlib/full/textwrap/run.py \
  --imre target/release/imre --runtime target/release/libimre_runtime.a \
  --clang /usr/bin/clang-18 --cpython-source /path/to/cpython-3.14.5 \
  --result /tmp/textwrap-native.json
```
