"""Exercise request cleanup and the joinable thread list of the boxed TCP server."""
import socket
from _thread import allocate_lock
from socketserver import BaseRequestHandler, ThreadingTCPServer


class Reply(BaseRequestHandler):
    def handle(self):
        self.request.sendall(b"reply")


with ThreadingTCPServer(("127.0.0.1", 0), Reply) as server:
    with socket.socket() as client:
        client.connect(server.server_address)
        request, address = server.get_request()
        server.process_request_thread(request, address)
        assert client.recv(5) == b"reply" and client.recv(1) == b""


def acquire_on_worker(started, gate):
    started.acquire()
    gate.acquire()


class WorkerServer(ThreadingTCPServer):
    process_request_thread = staticmethod(acquire_on_worker)


started, gate = allocate_lock(), allocate_lock()
gate.acquire()
with WorkerServer(("127.0.0.1", 0), Reply) as server:
    try:
        server.process_request(started, gate)
        worker, = server._threads
        assert worker.is_alive() and not worker.daemon
    finally:
        gate.release()
assert started.locked() and gate.locked() and not worker.is_alive()
print("request closed and non-daemon worker joined")
