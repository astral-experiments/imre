# urllib.parse

Imre splits a string or an ASCII byte URL into the five fields of a `SplitResult` or `SplitResultBytes`. It preserves the authority's spelling and exposes `username`, `password`, `hostname` and the lazily validated `port`. The result supports named-field and positional access, tuple unpacking, `geturl()`, and the corresponding `encode()` or `decode()` conversion. `urllib.parse.unquote` supports string input with its default UTF-8 replacement behavior. These are the entry points in this slice. The `urlsplit` cache and its inspection methods, calls on arbitrary user-defined objects, and other URL parser entry points remain open.

The complete CPython 3.14.5 `Lib/test/test_urlparse.py` is vendored verbatim alongside its license. The reference discovered and ran **77 methods**. Its five skips belong to non-ASCII byte subtests across three methods. All other subtests passed. The selected original methods and the exact fixture skips are in [the Linux run](urlsplit-linux.json).

| Execution in ordinary and reachable builds at O0 and O3 | Result |
| --- | --- |
| Unchanged `test_invalid_bracketed_hosts` method and assertions, original `TestCase` base through Imre's `unittest`, native operations | One method passes. All 43 assertions have corresponding native `urlsplit` calls in O0 IR. |
| Original CPython test methods, fixtures and assertions on CPython, Imre supplying split fields, authority properties, `geturl` and errors | Seven methods pass, including the bracketed-host method above. The strip test also uses CPython's standalone `urlunsplit`. |
| Unchanged Werkzeug `WSGIRequestHandler.make_environ` called on CPython with native URL results | Five request targets have matching path, query, host and raw URI, including an absolute target and a target beginning with two slashes. |

The two consumer groups exercise 184 distinct calls per mode. Imre directly runs the [focused Python controls](../../../../tests/fixtures/text/urlsplit.py). The complete native module import and the remainder of the unchanged test suite have separate compiler dependencies. A whole-suite coverage percentage would conflate the origins of these assertions.

Run the comparison with a test-enabled CPython 3.14.5 and the wheels pinned in `benchmarks/flask/requirements.txt`.

```sh
python3.14 -B benchmarks/cpython_stdlib/full/urllib/run_urlsplit.py \
  --compiler /path/to/imre --runtime /path/to/libimre_runtime.a \
  --wheelhouse /path/to/wheels --output /tmp/urllib-results.json
```
