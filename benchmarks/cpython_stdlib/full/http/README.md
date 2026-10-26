# HTTP status and method enums

The bundled `http` root uses CPython 3.14.5's complete module, including its member declarations and category properties. A status has an integer value and its own identity. Numeric and name lookup return the same canonical member, including the four legacy names. Iterating `HTTPStatus` visits 62 members. Iterating `HTTPStatus.__members__.values()` visits all 66 declarations in order.

```python
from http import HTTPMethod, HTTPStatus

status = HTTPStatus(404)
assert status is HTTPStatus.NOT_FOUND
assert isinstance(status, int) and status.value == 404
assert status.phrase == "Not Found"
assert status.description == "Nothing matches the given URI"
assert status.is_client_error
assert HTTPStatus["REQUEST_ENTITY_TOO_LARGE"] is HTTPStatus.CONTENT_TOO_LARGE
assert HTTPMethod("GET") is HTTPMethod.GET
```

The [public operations](operations.py) compare integer conversion, type checks, canonical identity, names, values, phrases, descriptions, errors and all five category properties with ordinary CPython. They also exercise the nine `HTTPMethod` members and their string values. The [consumer probe](responses.py) uses the exact `BaseHTTPRequestHandler.responses` comprehension extracted from the [upstream source](upstream/Lib/http/server.py). The resulting dictionary retains all 62 enum keys and resolves equal integers. Missing integer subscripts preserve the original `KeyError` argument.

The [Werkzeug fixture](werkzeug/inputs.py) wraps the byte-identical `Response._clean_status` method and its status-code table from Werkzeug 3.1.8. It covers status members, known and unknown integers, text statuses and the empty-status error. CPython also runs the 11 inputs against the actual wheel. The native fixture exercises the method in a minimal `Response` class. Flask application startup, HTTP routing and serving are separate workloads. The [wheel receipt](werkzeug/UPSTREAM.json) pins the artifact, source files, excerpt lines and license. All three direct comparisons run at native O0 and O3 in ordinary and reachable modes. Their [results](api-linux.json) are separate from original suite credit.

## Original Linux tests

The reference uses CPython [`5607950`](https://github.com/python/cpython/tree/5607950ef232dad16d75c0cf53101d9649d89115). The complete four original test modules, the five `http` implementation files and the [PSF license](upstream/LICENSE) are present as unchanged Git blobs. The per-suite manifests pin their identities. The table counts discovered occurrences at each optimization level. A fully native occurrence requires its original body, assertions, operation and fixture to execute on Imre.

| Original suite | Occurrences | Fully native O0 and O3 | CPython body and assertions, native operation | Blocked | Reference skips |
| --- | ---: | ---: | ---: | ---: | ---: |
| [test_httplib](evidence-httplib-linux.json) | 144 | 0 | 2 | 137 | 5 |
| [test_httpservers](evidence-httpservers-linux.json) | 92 | 0 | 0 | 92 | 0 |
| [test_http_cookies](evidence-http_cookies-linux.json) | 33 | 0 | 0 | 32 | 1 |
| [test_http_cookiejar](evidence-http_cookiejar-linux.json) | 77 | 0 | 0 | 77 | 0 |
| Total | 346 | 0 | 2 | 338 | 6 |

All four `test_httplib.BasicTest` originals specific to `HTTPStatus` pass on ordinary CPython. For `test_httpstatus_range` and `test_httpstatus_category`, CPython runs the original bodies, assertions and fixture, while a native worker performs the comparisons and category queries. The full native staging attempt requires the module's original `TestCase` binding for these methods and for `test_dir_with_added_behavior_on_status`. Staging `test_simple_httpstatus` stops at its nested check class and the surrounding class state. `HttpMethodTests.test_invalid_method_names` exercises `HTTPConnection`.

The client, server and cookies suites reach imports for their respective HTTP submodules. The cookiejar suite reaches a static rebinding in `test.support.os_helper`. The [startup and method receipts](measurements-linux.json) retain the diagnostics from all four native configurations. Four HTTPS cases require the network resource, one requires walltime and one cookie case requires the CPU resource. The occurrence ledgers retain their original conditions and locations. `test_httpservers` is a single shared denominator for work on that module.

## Reproduce

Use the pinned ordinary CPython with its test package and built native compiler and runtime. For the actual-wheel check, provide the verified Werkzeug 3.1.8 wheel and put its MarkupSafe dependency on the reference interpreter's import path.

```sh
suite=benchmarks/cpython_stdlib/full/http
PYTHONPATH="$markupsafe_wheel" "$python314" -I -B "$suite/verify.py" \
    --compiler "$imre" --runtime "$runtime" --clang "$clang" \
    --wheel "$werkzeug_wheel" --output "$output/api-linux.json"
"$python314" -I -B "$suite/coverage.py" --compiler "$imre" --runtime "$runtime" \
    --clang "$clang" --output "$output"
```
