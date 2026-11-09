"""A Python send override may change its receiver before delegating a write."""
import io
import os
import socket


class Writer(socket.socket):
    def send(self, data):
        self.close()
        self.__init__(fileno=os.dup(other.fileno()))
        self.sendall(data)
        return len(data)


left, original = socket.socketpair()
other, replacement = socket.socketpair()
with Writer(fileno=left.detach()) as writer, original, other, replacement:
    try:
        writer.sendfile(io.BytesIO(b"private"))
    except OSError:
        pass
    else:
        raise AssertionError("file transfer changed socket")
    assert original.recv(1) == b""
    writer.sendall(b"done")
    writer.shutdown(socket.SHUT_WR)
    assert replacement.recv(32, socket.MSG_WAITALL) == b"done"
