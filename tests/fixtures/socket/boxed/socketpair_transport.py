import _socket
import array
import errno
import io
import os
import socket
from socket import fromfd, socketpair


left, right = socketpair()
with left, right, open(__file__, "rb") as source:
    source.seek(12)
    assert left.sendfile(source, offset=1, count=3) == 3
    assert source.tell() == 4
    with io.BytesIO(b"buffer") as stream:
        assert left.sendfile(stream, 1, 2) == 2
        assert stream.tell() == 3
    left.shutdown(socket.SHUT_WR)
    assert right.recv(5, socket.MSG_WAITALL) == b"mpouf"
    assert right.recv(1) == b""


left, right = socketpair()
with left, right:
    for endpoint in (left, right):
        assert endpoint.family == socket.AF_UNIX
        assert endpoint.type == socket.SOCK_STREAM and endpoint.proto == 0
        assert endpoint.fileno() >= 0 and not endpoint.get_inheritable()
        assert endpoint.getsockname() == endpoint.getpeername() == ""
    assert left.fileno() != right.fileno()
    assert left.set_inheritable(inheritable=True) is None
    assert os.get_inheritable(left.fileno())
    left.set_inheritable(False)
    assert not left.get_inheritable()
    left.sendall(b"left")
    assert right.recv(4) == b"left"
    right.sendall(b"right")
    assert left.recv(5) == b"right"
    left.shutdown(socket.SHUT_WR)
    assert right.recv(1) == b""
    left.close()
    assert left.fileno() == -1 and right.fileno() >= 0
    try:
        left.get_inheritable()
    except OSError as exc:
        assert exc.errno == errno.EBADF
    else:
        raise AssertionError("read descriptor flags after close")
assert right.fileno() == -1

# A release callback can reuse an earlier exporter's storage once its view is gone.
def check_vector_release(retain):
    page = bytearray(b"A")
    saved = []
    released = []

    class Page:
        def __buffer__(self, flags):
            view = memoryview(page)
            if retain:
                saved.append(view)
            return view

    class Trailer:
        def __buffer__(self, flags):
            return memoryview(b"B")

        def __release_buffer__(self, view):
            try:
                page.extend(b"!")
            except BufferError:
                released.append(False)
            else:
                released.append(True)

    left, right = socketpair()
    with left, right:
        assert left.sendmsg([Page(), Trailer()]) == 2
        assert right.recv(2) == b"AB"
    assert released == [not retain]
    assert page == (b"A" if retain else b"A!")


check_vector_release(False)
check_vector_release(True)


# Linux sendmsg uses the caller's flags and current SIGPIPE disposition.
if hasattr(socket, "MSG_NOSIGNAL"):
    import signal
    import sys

    if sys.platform == "linux":
        delivered = []
        previous = signal.signal(signal.SIGPIPE, lambda signum, frame: delivered.append(signum))
        try:
            left, right = socketpair()
            right.close()
            with left:
                for flags in (socket.MSG_NOSIGNAL, 0):
                    try:
                        left.sendmsg([b"x"], [], flags)
                    except BrokenPipeError as exc:
                        assert exc.errno == errno.EPIPE
                    else:
                        raise AssertionError("write to closed socketpair")
            assert delivered == [signal.SIGPIPE]
        finally:
            signal.signal(signal.SIGPIPE, previous)


# Transfer two data ranges and retain the received file after the sender closes its copy.
left, right = socketpair()
with left, right:
    with open(__file__, "rb") as source:
        descriptors = array.array("i", [source.fileno()])
        assert left.sendmsg([b"he", b"llo"], [
            (socket.SOL_SOCKET, socket.SCM_RIGHTS, descriptors)
        ]) == 5
    target = bytearray(b"-------")
    count, control, flags, _ = right.recvmsg_into(
        [memoryview(target)[1:3], memoryview(target)[3:6]],
        socket.CMSG_SPACE(descriptors.itemsize),
    )
    assert count == 5 and target == b"-hello-" and flags == 0
    [(level, kind, data)] = control
    assert (level, kind) == (socket.SOL_SOCKET, socket.SCM_RIGHTS)
    received = array.array("i")
    received.frombytes(data)
    [fd] = received
    try:
        assert os.read(fd, 6) == b"import"
    finally:
        os.close(fd)
    right.sendmsg([b"O", b"K"])
    assert left.recvmsg(2)[:3] == (b"OK", [], 0)

left, right = socketpair(family=socket.AF_UNIX, type=socket.SOCK_DGRAM)
with left, right:
    left.send(b"first")
    left.send(b"")
    left.send(b"last")
    assert right.recv(2) == b"fi"
    assert right.recv(10) == b""
    assert right.recv(10) == b"last"

left, right = socketpair()
with right, left.dup() as copy:
    old_fd = left.fileno()
    left.close()
    # Reuse the exact number for the peer before closing and dropping the old wrapper.
    os.dup2(right.fileno(), old_fd, inheritable=False)
    try:
        left.close()
        del left
        os.write(old_fd, b"x")
        assert copy.recv(1) == b"x"
    finally:
        os.close(old_fd)

left, right = socketpair()
with left, right:
    left.settimeout(1)
    with fromfd(left.fileno(), left.family, left.type) as copy:
        assert copy.fileno() != left.fileno() and not copy.get_inheritable()
        assert copy.gettimeout() == socket.getdefaulttimeout()
        left.close()
        right.sendall(b"x")
        right.shutdown(socket.SHUT_WR)
        assert copy.recv(1) == b"x" and copy.recv(1) == b""

previous = socket.getdefaulttimeout()
try:
    socket.setdefaulttimeout(0.75)
    left, right = socketpair()
    with left, right:
        assert left.gettimeout() == right.gettimeout() == 0.75
        socket.setdefaulttimeout(None)
        with socket.socket() as other:
            assert other.gettimeout() is None and left.gettimeout() == 0.75
finally:
    socket.setdefaulttimeout(previous)


finalized = []


class QuietOwner:
    def __del__(self):
        finalized.append(True)


def peer_and_holder():
    peer, endpoint = _socket.socketpair()
    peer.setblocking(False)
    return peer, [endpoint]


def check_last_endpoint(keep):
    peer, holder = peer_and_holder()
    retained = holder.copy() if keep else []
    # Let the runtime learn the paths to the unrelated finalizers before discarding the socket.
    for i in range(160):
        assert owners[i % 10] is not None
    try:
        received = holder.pop() and peer.recv(1)
    except BlockingIOError:
        assert retained
        assert retained[0].send(b"x") == 1
        assert peer.recv(1) == b"x"
        retained[0].close()
        assert peer.recv(1) == b""
    else:
        assert not retained and received == b""
    assert not finalized
    peer.close()


owners = [QuietOwner() for _ in range(10)]
check_last_endpoint(False)
check_last_endpoint(True)
owners.clear()
