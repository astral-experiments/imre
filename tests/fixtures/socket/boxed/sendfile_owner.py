"""Closing a sender during file I/O must never redirect its bytes to a reused fd."""

import io
import os
import socket


def check(buffered):
    sender, expected = socket.socketpair()
    foreign, unexpected = socket.socketpair()
    target = sender.fileno()

    def replace_sender():
        sender.close()
        os.dup2(foreign.fileno(), target)

    with sender, expected, foreign, unexpected, open(__file__, "rb") as source:
        class File:
            def fileno(self):
                if buffered:
                    raise io.UnsupportedOperation()
                replace_sender()
                return source.fileno()

            def read(self, n):
                if buffered:
                    replace_sender()
                return source.read(n)

            def seek(self, offset):
                return source.seek(offset)

        try:
            try:
                sender.sendfile(File(), count=4)
            except OSError:
                pass
            else:
                raise AssertionError("sent from a closed socket")
            assert expected.recv(1) == b""
            unexpected.setblocking(False)
            try:
                unexpected.recv(1)
            except BlockingIOError:
                pass
            else:
                raise AssertionError("wrote to another socket")
        finally:
            if sender.fileno() < 0:
                os.close(target)


def check_restored_descriptor():
    sender, expected = socket.socketpair()
    foreign, unexpected = socket.socketpair()
    fd = sender.fileno()
    with sender, expected, foreign, unexpected, open(__file__, "rb") as source:
        reserved = os.dup(fd)
        os.close(reserved)

        class File:
            def fileno(self):
                sender.close()
                os.dup2(foreign.fileno(), fd)
                os.dup2(foreign.fileno(), reserved)
                return source.fileno()

            def read(self, size):
                return source.read(size)

            def seek(self, position):
                return source.seek(position)

        try:
            try:
                sender.sendfile(File(), count=4)
            except OSError:
                pass
            else:
                raise AssertionError("accepted a replacement transport")
            assert expected.recv(1) == b""
            assert os.write(reserved, b"G") == 1
            assert unexpected.recv(5) == b"G"
        finally:
            if sender.fileno() < 0:
                os.close(fd)
                os.close(reserved)


def check_reattached_socket(detach):
    sender, peer = socket.socketpair()
    with sender, peer:
        data = io.BytesIO(b"private")

        class Source:
            def read(self, n):
                fd = sender.detach() if detach else sender.fileno()
                socket.socket.__init__(sender, fileno=fd)
                return data.read(n)

        source = Source()
        if detach:
            try:
                sender.sendfile(source, count=7)
            except OSError as exc:
                assert exc.errno == 9
            else:
                raise AssertionError("sent file bytes after descriptor ownership was transferred")
        else:
            assert sender.sendfile(source, count=7) == 7
            assert peer.recv(7) == b"private"
        sender.sendall(b"own")
        assert peer.recv(3) == b"own"


check(False)
check(True)
check_restored_descriptor()
check_reattached_socket(True)
check_reattached_socket(False)
