"""Synchronous IPv4 TCP servers using native socket objects."""
from __future__ import annotations

from _imre_socketserver_reader import _SocketReader
from _imre_socketserver_poll import _open_poll, _poll_ready, _close_poll
import socket
import sys


class BaseRequestHandler:
    request: socket.socket
    client_address: tuple[str, int]
    server: TCPServer

    def __init__(self, request: socket.socket, client_address: tuple[str, int], server: TCPServer) -> None:
        self.request = request
        self.client_address = client_address
        self.server = server
        _run_handler(self)

    def setup(self) -> None:
        pass

    def handle(self) -> None:
        pass

    def finish(self) -> None:
        pass


def _run_handler(handler: BaseRequestHandler) -> None:
    handler.setup()
    try:
        handler.handle()
    finally:
        handler.finish()


class _SocketWriter:
    _socket: socket.socket
    closed: bool

    def __init__(self, sock: socket.socket) -> None:
        self._socket = sock
        self.closed = False

    def write(self, data: bytes) -> int:
        self._socket.sendall(data)
        return len(data)

    def writable(self) -> bool:
        return True

    def fileno(self) -> int:
        return self._socket.fileno()

    def flush(self) -> None:
        if self.closed:
            raise ValueError("I/O operation on closed file.")

    def close(self) -> None:
        if not self.closed:
            try:
                self.flush()
            finally:
                self.closed = True


class StreamRequestHandler(BaseRequestHandler):
    timeout = None
    rbufsize = -1
    wbufsize = 0
    disable_nagle_algorithm = False
    _connection: socket.socket | None
    # A populated slot can hold None in runtime-loaded Python.
    _reader: tuple[_SocketReader] | None
    _writer: tuple[_SocketWriter] | None

    def __init__(self, request: socket.socket, client_address: tuple[str, int], server: TCPServer) -> None:
        self._connection = None
        self._reader = None
        self._writer = None
        super().__init__(request, client_address, server)

    def setup(self) -> None:
        self.connection = self.request
        if self.timeout is not None:
            raise NotImplementedError("socketserver stream timeout only supports None")
        if self.rbufsize != -1 or self.wbufsize != 0:
            raise NotImplementedError("custom socketserver stream buffer sizes are unsupported")
        if self.disable_nagle_algorithm:
            self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.rfile = _SocketReader(self.connection)
        self.wfile = _SocketWriter(self.connection)

    @property
    def connection(self) -> socket.socket:
        connection = self._connection
        if connection is None:
            raise AttributeError("connection")
        return connection

    @connection.setter
    def connection(self, value: socket.socket) -> None:
        self._connection = value

    @property
    def rfile(self) -> _SocketReader:
        reader = self._reader
        if reader is None:
            raise AttributeError("rfile")
        return reader[0]

    @rfile.setter
    def rfile(self, value: _SocketReader) -> None:
        self._reader = (value,)

    @property
    def wfile(self) -> _SocketWriter:
        writer = self._writer
        if writer is None:
            raise AttributeError("wfile")
        return writer[0]

    @wfile.setter
    def wfile(self, value: _SocketWriter) -> None:
        self._writer = (value,)

    def finish(self) -> None:
        if not self.wfile.closed:
            try:
                self.wfile.flush()
            except OSError:
                pass
        self.wfile.close()
        self.rfile.close()


class TCPServer:
    server_address: tuple[str, int]
    RequestHandlerClass: type[BaseRequestHandler]
    socket: socket.socket
    address_family = socket.AF_INET
    socket_type = socket.SOCK_STREAM
    allow_reuse_address = False
    allow_reuse_port = False
    timeout = None
    request_queue_size = 5
    _shutdown_requested: bool
    _server_stopped: bool

    def __init__(self, server_address: tuple[str, int], RequestHandlerClass: type[BaseRequestHandler], bind_and_activate: bool = True) -> None:
        self.server_address = server_address
        self.RequestHandlerClass = RequestHandlerClass
        self._shutdown_requested = False
        self._server_stopped = False
        self.socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        _activate(self, bind_and_activate)

    def server_bind(self) -> None:
        if self.allow_reuse_port:
            raise NotImplementedError("socketserver allow_reuse_port is unsupported")
        if self.allow_reuse_address:
            self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.socket.bind(self.server_address)
        self.server_address = self.socket.getsockname()

    def server_activate(self) -> None:
        self.socket.listen(self.request_queue_size)

    def fileno(self) -> int:
        return self.socket.fileno()

    def server_close(self) -> None:
        self.socket.close()

    def __enter__(self) -> TCPServer:
        return self

    def __exit__(self, exc_type: object, exc: object, traceback: object) -> None:
        self.server_close()

    def get_request(self) -> tuple[socket.socket, tuple[str, int]]:
        return self.socket.accept()

    def verify_request(self, request: socket.socket, client_address: tuple[str, int]) -> bool:
        return True

    def finish_request(self, request: socket.socket, client_address: tuple[str, int]) -> None:
        handler = self.RequestHandlerClass
        handler(request, client_address, self)

    def close_request(self, request: socket.socket) -> None:
        request.close()

    def shutdown_request(self, request: socket.socket) -> None:
        try:
            request.shutdown(socket.SHUT_WR)
        except OSError:
            pass
        self.close_request(request)

    def process_request(self, request: socket.socket, client_address: tuple[str, int]) -> None:
        self.finish_request(request, client_address)
        self.shutdown_request(request)

    def handle_error(self, request: socket.socket, client_address: tuple[str, int]) -> None:
        sys.stderr.write("----------------------------------------\n")
        sys.stderr.write("Exception occurred during processing of request from " + repr(client_address) + "\n")
        exc = sys.exception()
        if exc is not None:
            sys.stderr.write(str(exc) + "\n")
        sys.stderr.write("----------------------------------------\n")

    def serve_forever(self, poll_interval: float | None = 0.5) -> None:
        self._server_stopped = False
        try:
            selector = _open_poll(self.fileno())
            try:
                while not self._shutdown_requested:
                    ready = _poll_ready(selector, poll_interval)
                    if self._shutdown_requested:
                        break
                    if ready:
                        self._handle_request_noblock()
                    self.service_actions()
            finally:
                _close_poll(selector)
        finally:
            self._shutdown_requested = False
            self._server_stopped = True

    def shutdown(self) -> None:
        self._shutdown_requested = True
        # Wait for the loop's finally block, including a request already being handled.
        waiter = _open_poll(None)
        try:
            while not self._server_stopped:
                _poll_ready(waiter, 0.01)
        finally:
            _close_poll(waiter)

    def service_actions(self) -> None:
        """Hook for work between requests and on idle polls."""

    def handle_request(self) -> None:
        if self.fileno() < 0:
            raise ValueError("Invalid file descriptor: -1")
        if self.timeout is not None:
            raise NotImplementedError("socketserver listener timeout only supports None")
        if not self.socket.getblocking():
            raise NotImplementedError("nonblocking listener requires selector integration")
        self._handle_request_noblock()

    def _handle_request_noblock(self) -> None:
        try:
            request, client_address = self.get_request()
        except OSError:
            return
        if self.verify_request(request, client_address):
            try:
                self.process_request(request, client_address)
            except Exception:
                self.handle_error(request, client_address)
                self.shutdown_request(request)
            except:
                self.shutdown_request(request)
                raise
        else:
            self.shutdown_request(request)


def _activate(server: TCPServer, bind_and_activate: bool) -> None:
    if server.address_family != socket.AF_INET or server.socket_type != socket.SOCK_STREAM:
        server.server_close()
        raise NotImplementedError("TCPServer requires AF_INET and SOCK_STREAM")
    if bind_and_activate:
        try:
            server.server_bind()
            server.server_activate()
        except:
            server.server_close()
            raise
