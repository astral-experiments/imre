"""A failed makefile must release only its own reference to the socket."""
import socket

with socket.socket() as listener, socket.socket() as client:
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    client.connect(listener.getsockname())
    peer, _ = listener.accept()
    descriptor = peer.fileno()
    first = peer.makefile("rb")
    alias = peer
    peer.close()
    try:
        candidate = alias.makefile("rb")
    except MemoryError as error:
        assert error.args == ()
        print("caught")
    else:
        candidate.close()
        print("opened")

    assert alias.fileno() == first.fileno() == descriptor
    retry = alias.makefile("rb")
    client.sendall(b"first\n")
    assert first.readline() == b"first\n"
    client.sendall(b"retry")
    client.shutdown(socket.SHUT_WR)
    assert retry.read() == b"retry"
    first.close()
    assert alias.fileno() == descriptor
    retry.close()
    assert alias.fileno() == -1
    retry.close()
