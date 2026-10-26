"""Exercise the selector used by runtime-loaded transports on two inherited sockets."""
import selectors
import sys

# Exercise runtime-loaded imports just as a program inspecting its search path would.
sys.path


class SocketView:
    def __init__(self, fd):
        self.fd = fd

    def fileno(self):
        return self.fd


class Broken:
    def fileno(self):
        raise RuntimeError("fileno failed")


def register_temporary(selector, fd):
    selector.register(SocketView(fd), selectors.EVENT_READ, ["retained"])


fd = int(sys.argv[1])
idle = int(sys.argv[2])
peer = SocketView(fd)
data = ["connection"]
with selectors.DefaultSelector() as selector:
    key = selector.register(peer, selectors.EVENT_READ, data)
    idle_key = selector.register(idle, selectors.EVENT_READ)
    assert key.fileobj is peer and key.fd == fd and key.data is data
    assert isinstance(key, selectors.SelectorKey) and isinstance(key, tuple)
    assert key[0] is peer and key[1] == fd and key[2] == selectors.EVENT_READ
    try:
        key.events = selectors.EVENT_WRITE
    except AttributeError:
        pass
    else:
        raise AssertionError("key events are writable")
    assert selector.get_key(fd) is key and selector.get_key(idle) is idle_key
    assert selector.modify(fd, selectors.EVENT_READ, ["connection"]) is key
    if sys.argv[3] == "idle":
        assert selector.select(timeout=0.01) == []
    else:
        if sys.argv[3] == "delayed":
            print("waiting", flush=True)
            ready = selector.select(timeout=1)
        else:
            ready = selector.select(timeout=0.01)
        assert len(ready) == 1 and ready[0][0] is key and ready[0][1] == selectors.EVENT_READ
    updated = selector.modify(fileobj=fd, events=selectors.EVENT_WRITE, data="writer")
    assert updated is not key and updated.fileobj is peer and updated.data == "writer"
    ready = selector.select(0)
    assert len(ready) == 1 and ready[0][0] is updated and ready[0][1] == selectors.EVENT_WRITE
    try:
        selector.register(Broken(), 0)
    except ValueError:
        pass
    else:
        raise AssertionError("invalid interest was accepted")
    try:
        selector.register(Broken(), selectors.EVENT_READ)
    except RuntimeError as e:
        assert str(e) == "fileno failed"
    else:
        raise AssertionError("fileno error was lost")
    try:
        selector.register(fd, selectors.EVENT_READ)
    except KeyError:
        pass
    else:
        raise AssertionError("duplicate fd was accepted")
    peer.fd = -1
    assert selector.unregister(peer) is updated and selector.unregister(idle) is idle_key
    assert selector.select(-0.1) == [] and selector.select(-(1 << 1100)) == []
    register_temporary(selector, idle)
    # Keep the selector as the only owner of this key across native heap collections.
    for i in range(1500):
        garbage = [i]
    retained = selector.get_key(idle)
    assert retained.fileobj.fileno() == idle and retained.data == ["retained"]
try:
    selector.get_key(fd)
except RuntimeError:
    pass
else:
    raise AssertionError("get_key on closed selector was accepted")
print("boxed selector file objects passed")
