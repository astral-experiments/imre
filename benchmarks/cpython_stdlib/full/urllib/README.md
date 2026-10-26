# urllib.parse

Imre splits a string or an ASCII byte URL into the five fields of a `SplitResult` or `SplitResultBytes`. It preserves the authority's spelling and exposes `username`, `password`, `hostname` and the lazily validated `port`. The result supports named-field and positional access, tuple unpacking, `geturl()`, and the corresponding `encode()` or `decode()` conversion. `urllib.parse.unquote` supports string input with its default UTF-8 replacement behavior. These are the entry points in this slice. The `urlsplit` cache and its inspection methods, calls on arbitrary user-defined objects, and other URL parser entry points remain open.

The complete CPython 3.14.5 `Lib/test/test_urlparse.py` is vendored verbatim alongside its license. **All 77 reference methods passed.** Five non-ASCII byte subtests skipped across three methods that also had passing subtests. The reference retains their exact parameters and reasons. [Linux reference](reference-urlparse-linux.json)

| Execution in ordinary and reachable builds at O0 and O3 | Result |
| --- | --- |
| Unchanged `test_invalid_bracketed_hosts` method and assertions, original `TestCase` base through Imre's `unittest`, native operations | One method passes. All 43 assertions have corresponding native `urlsplit` calls in O0 IR. |
| Original methods, fixtures and assertions on CPython, Imre supplying split fields, authority properties, `geturl` and errors | Five further methods pass with Imre providing the product operation. A sixth, `test_urlsplit_strip_url`, also calls CPython's standalone `urlunsplit`. |
| Unchanged Werkzeug `WSGIRequestHandler.make_environ` called on CPython with native URL results | Five request targets have matching path, query, host and raw URI, including an absolute target and a target beginning with two slashes. |

The bracketed-host method counts once. The remaining 70 parser methods have CPython reference results only. The two consumer groups exercise 184 distinct calls per mode. The five Werkzeug environments and the focused controls are separate from the upstream denominator. [Per-method O0/O3 evidence](evidence-urlparse-linux.json) · [Four-mode observations](urlsplit-linux.json) · [Focused controls](../../../../tests/fixtures/text/urlsplit.py)

### Companion scope

The measured denominator is the 77 parser methods. The same pinned reference interpreter discovers 234 additional occurrences across six companions. Their method outcomes and native compatibility are unmeasured here. With default resources the two remote-network suites skip at import. Enabling the network resource for discovery exposes their IDs without running their tests.

| CPython entry | Discovered methods |
| --- | ---: |
| `test_urllib` | 101 |
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
python3.14 -B benchmarks/cpython_stdlib/full/urllib/run_urlsplit.py \
  --compiler /path/to/imre --runtime /path/to/libimre_runtime.a \
  --wheelhouse /path/to/wheels --output /tmp/urllib-results.json \
  --evidence /tmp/evidence-urlparse-linux.json
```
