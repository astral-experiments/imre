# ast.literal_eval

Jinja 3.1.6 imports `literal_eval` at lexer.py:9. Its float-token branch removes underscores and passes the result to the imported function at line 663. Rendering `{{ 1_0.5 }}` in the unchanged wheel makes one call with `"10.5"`. The ten reference requests to the seven-wheel Flask app make zero calls, although startup imports the lexer. [Jinja lexer](https://github.com/pallets/jinja/blob/3.1.6/src/jinja2/lexer.py#L663)

The standalone runtime parses text with its existing Rust Python parser and converts the resulting mutable AST. Names and call targets in the source are never looked up. The sole accepted call shape is an argument-free `set()`. Negative tests include a chained import and call, a lambda, a comprehension, nested signs and arithmetic outside the real-plus-imaginary literal form.

## Original CPython tests

Run the test-enabled CPython 3.14.5 interpreter with the pinned source checkout and a matching Imre compiler/runtime pair.

```sh
"$CPYTHON" -B benchmarks/cpython_stdlib/full/ast/literal_eval.py \
  --cpython-source "$CPYTHON_SRC" --compiler "$IMRE" --runtime "$RUNTIME" \
  --clang "$CLANG" --output /tmp/ast-literal-eval.json
```

The driver verifies the license, test source and companion files against Git blobs at `5607950ef232dad16d75c0cf53101d9649d89115`. It discovers all nine methods in that file which reference `ast.literal_eval`, including its use as an `assertRaises` callback. They issue 48 calls at each of O0 and O3. The original fixture, test body and assertions run in CPython. Every `literal_eval` operation runs in a separate native executable. For hand-built AST arguments, the driver reconstructs the original fields and positions in that executable. CPython supplies its surrounding exception context in `test_parse_in_error`. [Original CPython tests](https://github.com/python/cpython/blob/5607950ef232dad16d75c0cf53101d9649d89115/Lib/test/test_ast/test_ast.py)

## Limits

Decimal integer literals use the process's integer conversion limit, including the configured and unlimited settings. Binary, octal and hexadecimal literals are exempt. Text with more than 200 nested grouping delimiters raises `SyntaxError`. The native guard also raises `RecursionError` for more than 512 prefix, power or conditional operators between comma-separated items, or 256 levels of hand-built literal nodes. The specific tree depth at which CPython exhausts its recursion budget varies.

For valid nonliteral syntax outside Imre's public AST projection, the error identifies the node kind and line with an abbreviated field display. Some other parser errors use a generic syntax diagnostic. These paths reject the input before executing it.
