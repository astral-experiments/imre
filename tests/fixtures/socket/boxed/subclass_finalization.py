"""A Python socket finalizer can hand an fd to another owner before resurrection."""
import gc
import os
import socket


def check_reclaimed_descriptor(detach):
    saved = []
    replacements = []

    class Session(socket.socket):
        def __del__(self):
            fd = self.detach() if detach else self.fileno()
            os.close(fd)
            replacement = os.open(os.devnull, os.O_RDONLY)
            if replacement != fd:
                os.dup2(replacement, fd)
                os.close(replacement)
            replacements.append(fd)
            saved.append(self)

    original, peer = socket.socketpair()
    fd = original.detach()
    try:
        session = Session(fileno=fd)
        session.cycle = session
        del session
        gc.collect()
        assert replacements == [fd] and len(saved) == 1
        assert peer.recv(1) == b"" and os.read(fd, 1) == b""
        saved.clear()
        gc.collect()
        os.fstat(fd)
        os.close(fd)
    finally:
        peer.close()


check_reclaimed_descriptor(False)
check_reclaimed_descriptor(True)


def check_audited_subtype():
    import sys

    events = []

    class Listener(socket.socket):
        pass

    def observe(event, args):
        if event == "socket.__new__" and isinstance(args[0], Listener):
            events.append((args[0].fileno(), *args[1:]))

    sys.addaudithook(observe)
    listener = Listener(socket.AF_INET, socket.SOCK_STREAM, 0)
    try:
        listener.bind(("127.0.0.1", 0))
        assert listener.getsockname()[1] > 0
        assert events == [(-1, int(socket.AF_INET), int(socket.SOCK_STREAM), 0)]
    finally:
        listener.close()


check_audited_subtype()
