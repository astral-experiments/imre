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


# Pin the user's selected bound send after seeking and retrieving file.read.
events = []

class Sender(socket.socket):
    def __getattribute__(self, name):
        if name == "send":
            events.append("lookup-send")
        return super().__getattribute__(name)

class Source(io.BytesIO):
    def seek(self, offset):
        events.append("seek")
        return super().seek(offset)

    def __getattribute__(self, name):
        if name == "read":
            events.append("lookup-read")
        return super().__getattribute__(name)

    def read(self, n):
        events.append("read")
        sender.send = other.send
        return super().read(n)

left, expected = socket.socketpair()
other, foreign = socket.socketpair()
with Sender(fileno=left.detach()) as sender, expected, other, foreign:
    assert sender.sendfile(Source(b".range"), 1, 5) == 5
    assert events == ["seek", "lookup-read", "lookup-send", "read", "seek"]
    assert expected.recv(5) == b"range"
    foreign.setblocking(False)
    try:
        foreign.recv(1)
    except BlockingIOError:
        pass
    else:
        raise AssertionError("file.read redirected the bound send")
