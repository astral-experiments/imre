"""Binary line reader for statically compiled socket handlers."""
import socket


class _SocketReader:
    _socket: socket.socket
    _pending: bytes
    closed: bool

    def __init__(self, sock: socket.socket) -> None:
        self._socket = sock
        self._pending = b""
        self.closed = False

    def readline(self, size: int = -1) -> bytes:
        if self.closed:
            raise ValueError("readline of closed file")
        n = 0
        done = False
        while True:
            while n < len(self._pending) and (size < 0 or n < size):
                n += 1
                if self._pending[n - 1] == 10:
                    done = True
                    break
            if done or n == size:
                break
            chunk = self._socket.recv(8192)
            if not chunk:
                break
            self._pending = self._pending + chunk
        line = self._pending[:n]
        self._pending = self._pending[n:]
        return line

    def close(self) -> None:
        self.closed = True
        self._pending = b""
