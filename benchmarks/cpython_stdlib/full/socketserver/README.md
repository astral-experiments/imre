# Synchronous TCPServer

The bundled `socketserver` module runs Python handler subclasses on a
blocking POSIX IPv4 listener. `TCPServer(address, Handler)` binds and listens.
`server.server_address` contains the assigned port. Connect a client, call
`handle_request()` once, then call `server_close()` or leave a `with` block.
`fileno()` returns the listener descriptor, or -1 after close. A selector
accepting file objects can register the server and poll this descriptor.
`serve_forever(poll_interval=0.5)` polls the listener and calls `service_actions()`
after processing a request or on an idle timeout. `shutdown()` waits for this loop
to exit. A later call can reuse the same listener.

Overridable server hooks bind, activate, accept, verify, process, finish,
report errors and close. Handlers receive the accepted socket, peer address
and actual server. Setup, handle and finish dispatch on the concrete class.
Shutdown half-closes the response and closes the socket even if a handler
retains a reference. Supported settings include `bind_and_activate`,
`allow_reuse_address` and `request_queue_size`. `StreamRequestHandler`
exposes the accepted socket as `connection`. Runtime-loaded handlers open its
buffered binary reader for `readline`, `read` and `readinto`. Checked handlers
use the existing native `readline([size])` reader. Both support unbuffered
`wfile.write(bytes)`, `flush()` and `close()`. The [two-client fixture](../../../../tests/fixtures/socketserver/boxed_stream.py)
checks the accepted socket against the selector key and reads the body through
the same buffered stream.

`handle_request()` requires a blocking listener and `timeout=None`. Threaded
and forking servers, Unix sockets and datagrams are outside this slice.
Custom stream buffer sizes and `allow_reuse_port=True` are explicitly unsupported.
The stream handler honors `disable_nagle_algorithm`.
The default error hook reports the peer and message
without Python traceback frames. Werkzeug also imports `http.server` and
defines classes using both socketserver mixins.

## Evidence

[UPSTREAM.json](UPSTREAM.json) records the CPython 3.14.5 commit and Git blob
IDs for the unchanged source, tests, support files and license. With the
network resource enabled, all **27** stock Linux cases pass. At both O0 and
O3, [evidence-linux.json](evidence-linux.json) records:

| Original method | Body and assertions | Native operation and negative control |
| --- | --- | --- |
| `test_context_manager` | Imre, with host TestCase fixture | Listener closes on exit. Omitting close fails the original fd assertion. |
| `test_tcpserver_bind_leak` | Imre, with host TestCase fixture | 1,024 failed binds do not leak descriptors. A leaking close reaches EMFILE under a 48-fd limit. |
| `test_TCPServer` | CPython, with original host thread and stream assertions | A native server echoes the three original requests. A wrong reply fails the original assertion. |

The other 24 are reference-only. There are no reference skips. The original
`test_TCPServer` uses a CPython thread to drive a bounded native `handle_request`
loop. The separate `serve_forever` fixture earns no original-suite credit. On
the integrated networking build, the unchanged test module reaches `import
threading` at line 11, which is unsupported. It also defines forking fixtures.

To reproduce with test-enabled CPython 3.14.5 and an Imre build:

```sh
python3.14 benchmarks/cpython_stdlib/suite_inventory.py benchmarks/cpython_stdlib/full/socketserver/UPSTREAM.json --result /tmp/socketserver-reference.json
cmp benchmarks/cpython_stdlib/full/socketserver/reference-linux.json /tmp/socketserver-reference.json
python3.14 benchmarks/cpython_stdlib/full/socketserver/run.py --compiler target/release/imre --runtime target/release/libimre_runtime.a --clang clang-18 --output /tmp/socketserver-evidence.json
cmp benchmarks/cpython_stdlib/full/socketserver/evidence-linux.json /tmp/socketserver-evidence.json
python3.14 tests/socketserver_tcp.py --compiler target/release/imre --runtime target/release/libimre_runtime.a --clang clang-18
```
