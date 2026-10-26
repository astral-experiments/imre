# HTTP status, method and server support

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

The [Werkzeug fixture](werkzeug/inputs.py) wraps the byte-identical `Response._clean_status` method and its status-code table from Werkzeug 3.1.8. It covers status members, known and unknown integers, text statuses and the empty-status error. CPython also runs the 11 inputs against the actual wheel. The native fixture exercises the method in a minimal `Response` class. On the isolated enum revision `bb6dbcdb`, the [five-request stock Flask gate](../../../flask/README.md) stopped at the `http.server` import in `werkzeug/serving.py:28` before serving any requests. The [wheel receipt](werkzeug/UPSTREAM.json) pins the artifact, source files, excerpt lines and license. All three direct comparisons run at native O0 and O3 in ordinary and reachable modes. Their [results](api-linux.json) are separate from original suite credit.

## Synchronous HTTP servers

`http.server.HTTPServer` binds a `socketserver.TCPServer`, records its qualified host name and actual port, and invokes the supplied handler class for each accepted connection. `BaseHTTPRequestHandler` uses `socketserver.StreamRequestHandler` streams. It parses request lines and headers, dispatches `do_*`, implements keep-alive and `100 Continue`, buffers response headers, and generates HTTP dates and error responses. HTTP/0.9 supplies an empty dictionary. HTTP/1.x uses `http.client.HTTPMessage` and preserves the spelling, order and repeated values of request headers. The class-level `responses` dictionary retains the original `HTTPStatus` keys.

```python
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, HTTPServer

class Health(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        self.send_response(HTTPStatus.NO_CONTENT)
        self.end_headers()

with HTTPServer(("127.0.0.1", 8080), Health) as server:
    server.handle_request()
```

The [subclass fixture](server_contract.py) observes delayed binding and request initialization, runs two dynamic command lookups on a Werkzeug-style handler, and checks duplicate headers and an overridden server version on real connections. The [original-method runner](server_cases.py) sends CPython's requests through an independent native listener. It substitutes a wrong status and a wrong date to check that the original assertions detect both errors.

## Original Linux tests

The reference uses CPython [`5607950`](https://github.com/python/cpython/tree/5607950ef232dad16d75c0cf53101d9649d89115). The complete four original test modules, the five `http` implementation files and the [PSF license](upstream/LICENSE) are present as unchanged Git blobs. The per-suite manifests pin their identities. The table counts discovered occurrences at each optimization level. A fully native occurrence requires its original body, assertions, operation and fixture to execute on Imre.

| Original suite | Occurrences | Fully native O0 and O3 | Native body, mixed fixture | CPython body and assertions, native operation | Blocked | Reference skips |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| [test_httplib](evidence-httplib-linux.json) | 144 | 0 | 0 | 2 | 137 | 5 |
| [test_httpservers](evidence-httpservers-linux.json) | 92 | 0 | 1 | 25 | 66 | 0 |
| [test_http_cookies](evidence-http_cookies-linux.json) | 33 | 0 | 0 | 0 | 32 | 1 |
| [test_http_cookiejar](evidence-http_cookiejar-linux.json) | 77 | 0 | 0 | 0 | 77 | 0 |
| Total | 346 | 0 | 1 | 27 | 312 | 6 |

All four `test_httplib.BasicTest` originals specific to `HTTPStatus` pass on ordinary CPython. For `test_httpstatus_range` and `test_httpstatus_category`, CPython runs the original bodies, assertions and fixture, while a native worker performs the comparisons and category queries. The full native staging attempt requires the module's original `TestCase` binding for these methods and for `test_dir_with_added_behavior_on_status`. Staging `test_simple_httpstatus` stops at its nested check class and the surrounding class state. `HttpMethodTests.test_invalid_method_names` exercises `HTTPConnection`.

All 22 original `BaseHTTPServerTestCase` clients and all three `HTTP09ServerTestCase` clients exercise the compiled server. `BaseHTTPRequestHandlerTestCase.test_date_time_string` runs its original body and unittest assertion inside a live native handler. A host TCP request triggers its fixture. The whole server suite stops importing `HTTPSServer` at line 7. Whole client suite startup reaches `threading` at line 10, and the cookies suite reaches its HTTP submodule import. The cookiejar suite reaches a static rebinding in `test.support.os_helper`. The [startup and method receipts](measurements-linux.json) retain the diagnostics from all four native configurations. Four HTTPS cases require the network resource, one requires walltime and one cookie case requires the CPU resource. The occurrence ledgers retain their original conditions and locations. `test_httpservers` is a single shared denominator for work on that module.

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
