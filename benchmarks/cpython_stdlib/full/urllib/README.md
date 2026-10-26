# urllib.parse

Imre splits a string or an ASCII byte URL into the five fields of a `SplitResult` or `SplitResultBytes`. It preserves the authority's spelling and exposes `username`, `password`, `hostname` and the lazily validated `port`. The result supports named-field and positional access, tuple unpacking, `geturl()`, and the corresponding `encode()` or `decode()` conversion. `quote` and `unquote` accept strings and bytes, positional or named codec arguments, and the error handlers supported by Imre’s codec registry. `quote` accepts a string or bytes safe set. The existing optimized one-argument string `unquote` still uses UTF-8 replacement. The `urlsplit` cache and its inspection methods, calls on arbitrary user-defined objects, and other URL parser entry points remain open.

The complete CPython 3.14.5 `Lib/test/test_urlparse.py` is vendored verbatim alongside its license. **All 77 reference methods passed.** Five non-ASCII byte subtests skipped across three methods that also had passing subtests. The reference retains their exact parameters and reasons. [Linux reference](reference-urlparse-linux.json)

| Execution in ordinary and reachable builds at O0 and O3 | Result |
| --- | --- |
| Unchanged `test_invalid_bracketed_hosts` method and assertions, original `TestCase` base through Imre's `unittest`, native operations | One method passes. All 43 assertions have corresponding native `urlsplit` calls in O0 IR. |
| Original methods, fixtures and assertions on CPython, Imre supplying split fields, authority properties, `geturl` and errors | Five further methods pass with Imre providing the product operation. A sixth, `test_urlsplit_strip_url`, also calls CPython's standalone `urlunsplit`. |
| Unchanged Werkzeug `WSGIRequestHandler.make_environ` called on CPython with native URL results | Five request targets have matching path, query, host and raw URI, including an absolute target and a target beginning with two slashes. |

The bracketed-host method counts once. The remaining 70 parser methods have CPython reference results only. The two consumer groups exercise 184 distinct calls per mode. The five Werkzeug environments and the focused controls are separate from the upstream denominator. [Per-method O0/O3 evidence](evidence-urlparse-linux.json) · [Four-mode observations](urlsplit-linux.json) · [Focused controls](../../../../tests/fixtures/text/urlsplit.py)

### Percent quoting

The full CPython 3.14.5 `Lib/test/test_urllib.py` companion contains 101 methods. On Linux, 99 pass and two Windows pathname tests skip. In ordinary and reachable Imre builds at O0 and O3, three unchanged test bodies and their assertions execute natively with a supplied isolated `TestCase` entry. CPython drives nine methods that also exercise other URL functions on CPython. The other 87 passing reference methods have reference results only. [Linux reference](reference-urllib-linux.json) · [Per-method execution roles](evidence-urllib-linux.json)

The tests exercise 406 distinct native calls in each mode, including every byte value. The unchanged Werkzeug 3.1.8 `dump_cookie`, `parse_dict_header`, and `parse_options_header` each produce five matching results when CPython runs the wrapper and Imre supplies the quoted text. Across these fifteen cases, the wheel calls `quote` four times and `unquote` nine times. The controls include a cookie path containing a prior escape, RFC 2231 continuation parameters, invalid escapes, and a charset the wheel declines to decode. [Observed methods and wheel outputs](quoting-linux.json) · [Regression fixture](../../../../tests/fixtures/text/urlquote.py)

`quote` preserves unreserved ASCII and the requested ASCII safe characters. It encodes a string through the native codec registry before quoting its octets. `unquote` decodes ASCII spans and preserves adjacent literal Unicode characters. Its bytes form decodes the whole buffer, including unescaped bytes. Current native codecs cover UTF-8, UTF-8 with signature, ASCII, Latin-1, CP1251, and CP424, with the aliases supplied by the registry. Other entry points such as `quote_plus`, `unquote_plus`, and `unquote_to_bytes` have separate implementations in CPython. The method ledger identifies when they provided part of an assertion.

### Companion scope

The measured denominator is 178 across `test_urlparse` and `test_urllib`. The same pinned interpreter discovers another 133 occurrences across the other five companions. Their method outcomes and native compatibility are unmeasured here. With default resources the two remote-network suites skip at import. Enabling the network resource for discovery exposes their IDs without running their tests.

| CPython entry | Discovered methods |
| --- | ---: |
| `test_urllib2` | 80 |
| `test_urllib2_localnet` | 21 |
| `test_urllib_response` | 4 |
| `test_urllib2net` | 16 |
| `test_urllibnet` | 12 |

To reproduce those counts on the full pinned CPython installation, load the suites without invoking their methods.

```python
import importlib
import unittest
from test import support

support.use_resources = ["network"]
for name in ("test_urllib", "test_urllib2", "test_urllib2_localnet",
             "test_urllib_response", "test_urllib2net", "test_urllibnet"):
    suite = unittest.defaultTestLoader.loadTestsFromModule(importlib.import_module("test." + name))
    print(name, suite.countTestCases())
```

Run the comparison with a test-enabled CPython 3.14.5 and the wheels pinned in `benchmarks/flask/requirements.txt`.

```sh
python3.14 -B benchmarks/cpython_stdlib/full/urllib/run_quoting.py \
  --compiler /path/to/imre --runtime /path/to/libimre_runtime.a \
  --wheelhouse /path/to/wheels --output /tmp/quoting-linux.json \
  --evidence /tmp/evidence-urllib-linux.json
python3.14 -B benchmarks/cpython_stdlib/full/urllib/run_urlsplit.py \
  --compiler /path/to/imre --runtime /path/to/libimre_runtime.a \
  --wheelhouse /path/to/wheels --output /tmp/urllib-results.json \
  --evidence /tmp/evidence-urlparse-linux.json
```
