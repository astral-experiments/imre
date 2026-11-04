import signal
import socket


payload = b"x" * (1024 * 1024)
drained = False


def interrupt(signum, frame):
    raise ZeroDivisionError("interrupted sendall")


def drain(signum, frame):
    global drained
    # Empty the queue. Bytes received after the timeout must come from a retry.
    while True:
        try:
            assert peer.recv(65536)
            drained = True
        except BlockingIOError:
            return


previous = signal.signal(signal.SIGALRM, interrupt)
try:
    sender, peer = socket.socketpair()
    with sender, peer:
        sender.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 4096)
        signal.alarm(1)
        try:
            sender.sendall(payload)
        except ZeroDivisionError as exc:
            assert str(exc) == "interrupted sendall"
        else:
            raise AssertionError("sendall ignored a raising handler")
        assert peer.recv(1) == b"x"

    sender, peer = socket.socketpair()
    with sender, peer:
        sender.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 4096)
        sender.settimeout(2.5)
        peer.setblocking(False)
        signal.signal(signal.SIGALRM, drain)
        signal.alarm(1)
        try:
            sender.sendall(payload)
        except TimeoutError:
            pass
        else:
            raise AssertionError("sendall lost its deadline")
        assert drained
        sender.close()
        assert peer.recv(1) == b"x"
finally:
    signal.alarm(0)
    signal.signal(signal.SIGALRM, previous)
