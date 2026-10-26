import errno
import os
import socket
import sys


def exercise(mode: str) -> None:
    if mode == "close" or mode == "close-reset":
        sock = socket.socket()
        alias = sock
        fd = sock.fileno()
        if mode == "close":
            try:
                sock.close()
            except InterruptedError as exc:
                assert exc.errno == errno.EINTR
            else:
                raise AssertionError("close did not surface EINTR")
        else:
            sock.close()
        assert alias.fileno() == -1
        alias.close()
        # This number now belongs to /dev/null. A close retry would destroy it.
        assert os.get_blocking(fd)
        return

    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        address = listener.getsockname()
        with socket.socket() as sender:
            if mode == "refused":
                try:
                    sender.connect(address)
                except ConnectionRefusedError as exc:
                    assert exc.errno == errno.ECONNREFUSED
                else:
                    raise AssertionError("interrupted connect lost SO_ERROR")
                return
            listener.listen()
            if mode == "nonblock":
                sender.setblocking(False)
                try:
                    sender.connect(address)
                except InterruptedError as exc:
                    assert exc.errno == errno.EINTR
                else:
                    raise AssertionError("nonblocking connect swallowed EINTR")
                conn, peer = listener.accept()
                conn.close()
                return
            sender.connect(address)
            receiver, peer = listener.accept()
            with receiver:
                data = b"\x00partial-writes\xff"
                sent = sender.send(data)
                assert sent == 3
                sender.sendall(data[sent:])
                sender.close()
                result = b""
                while True:
                    chunk = receiver.recv(7)
                    if not chunk:
                        break
                    result += chunk
                assert result == data


exercise(sys.argv[1])
print("ok")
