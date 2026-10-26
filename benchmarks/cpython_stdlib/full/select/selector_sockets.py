"""One idle TCP client must not hide another client's readiness or EOF."""
import selectors
import socket

with socket.socket() as listener, socket.socket() as idle_peer, socket.socket() as peer:
    listener.bind(("127.0.0.1", 0))
    listener.listen(2)
    idle_peer.connect(listener.getsockname())
    idle, _ = listener.accept()
    peer.connect(listener.getsockname())
    active, _ = listener.accept()
    with idle, active, selectors.DefaultSelector() as selector:
        idle_fd = idle.fileno()
        fd = active.fileno()
        idle_key = selector.register(idle_fd, selectors.EVENT_READ, "idle")
        read_key = selector.register(fd, selectors.EVENT_READ, "active")
        assert len(selector.select(0)) == 0, "idle sockets reported ready"

        assert peer.send(b"x") == 1
        ready = selector.select(1.0)
        assert len(ready) == 1 and ready[0][0] is read_key and ready[0][1] == selectors.EVENT_READ
        assert active.recv(1) == b"x"
        write_key = selector.modify(fd, selectors.EVENT_WRITE, "active")
        ready = selector.select(1.0)
        assert len(ready) == 1 and ready[0][0] is write_key and ready[0][1] == selectors.EVENT_WRITE, "write interest lost"
        assert active.send(b"y") == 1 and peer.recv(1) == b"y"
        assert selector.unregister(fd) is write_key
        assert len(selector.select(0)) == 0

        idle_peer.close()
        ready = selector.select(1.0)
        assert len(ready) == 1 and ready[0][0] is idle_key and ready[0][1] == selectors.EVENT_READ
        assert idle.recv(1) == b""
        assert selector.unregister(idle_fd) is idle_key and len(selector.select(0)) == 0
print("independent socket readiness passed")
