import os
import signal
import socket
import time


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


def wake_receiver(signum, frame):
    sender.send(b"R")


def expect_interruption(operation, queue):
    def handler(signum, frame):
        queue()
        raise RuntimeError("cancelled")

    signal.signal(signal.SIGALRM, handler)
    signal.alarm(1)
    try:
        operation()
    except RuntimeError as exc:
        assert str(exc) == "cancelled"
        assert exc.__context__ is None
    else:
        raise AssertionError("socket operation swallowed the signal")


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

    sender, peer = socket.socketpair()
    with sender, peer:
        peer.settimeout(3)
        buffer = bytearray(1)
        signal.signal(signal.SIGALRM, wake_receiver)
        signal.alarm(1)
        assert peer.recvmsg_into([buffer])[0] == 1
        assert buffer == b"R"

    sender, peer = socket.socketpair()
    with sender, peer:
        buffer = bytearray(b".")
        peer.settimeout(3)
        expect_interruption(lambda: peer.recv_into(buffer), lambda: sender.send(b"Q"))
        peer.setblocking(False)
        assert buffer == b"." and peer.recv(1) == b"Q"

    with socket.socket() as listener, socket.socket() as sender:
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        listener.settimeout(3)
        expect_interruption(listener.accept, lambda: sender.connect(listener.getsockname()))
        listener.setblocking(False)
        accepted, address = listener.accept()
        with accepted:
            assert address == sender.getsockname()

    if hasattr(os, "fork"):
        sender, receiver = socket.socketpair()
        with sender, receiver:
            notice, notify = os.pipe()
            pid = os.fork()
            if pid == 0:
                status = 1
                try:
                    os.close(notify)
                    for payload in (b"L", b"I", b"F", b"B"):
                        if os.read(notice, 1) != b"R":
                            break
                        # Let the interrupted receive encounter the still-empty socket.
                        time.sleep(0.05)
                        sender.sendall(payload)
                    else:
                        status = 0
                finally:
                    os._exit(status)
            os.close(notice)

            def change_timeout(signum, frame):
                receiver.settimeout(0.5)
                os.write(notify, b"R")

            try:
                signal.signal(signal.SIGALRM, change_timeout)
                signal.alarm(1)
                received = receiver.recv(1)
                assert received == b"L"
                receiver.setblocking(True)
                buffer = bytearray(b"...")
                signal.alarm(1)
                received = receiver.recv_into(memoryview(buffer)[1:2])
                assert received == 1 and buffer == b".I."
                receiver.setblocking(True)
                signal.alarm(1)
                assert receiver.recvfrom(1) == (b"F", None)
                receiver.setblocking(True)
                signal.alarm(1)
                assert receiver.recvfrom_into(memoryview(buffer)[1:2]) == (1, None)
                assert buffer == b".B."
            finally:
                signal.alarm(0)
                os.close(notify)
                _, status = os.waitpid(pid, 0)
            assert status == 0
finally:
    signal.alarm(0)
    signal.signal(signal.SIGALRM, previous)
