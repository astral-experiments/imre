"""Integer-fd API shared by the native and CPython checks."""
import errno
import os
import select
import sys


def track(p: select.poll, fd: int) -> None:
    p.register(fd, select.POLLIN)


def observed_mask(events: list[tuple[int, int]], fd: int) -> int:
    if len(events) == 0:
        return -1
    assert len(events) == 1 and events[0][0] == fd
    return events[0][1]


def temporary_interest(fd: int) -> None:
    p = select.poll()
    p.register(fd, select.POLLOUT)
    assert observed_mask(p.poll(0), fd) == select.POLLOUT


def borrowed_descriptors() -> None:
    reader, writer = os.pipe()
    replacement_reader, replacement_writer = os.pipe()
    p = select.poll()
    p.register(reader, select.POLLOUT)
    os.close(reader)
    os.dup2(replacement_writer, reader)
    assert observed_mask(p.poll(0), reader) == select.POLLOUT, "poll did not follow fd reuse"
    temporary_interest(reader)
    assert observed_mask(p.poll(0), reader) == select.POLLOUT, "dropping a poller closed its fd"
    p.unregister(reader)
    os.close(reader)
    os.close(writer)
    os.close(replacement_reader)
    os.close(replacement_writer)


def state() -> None:
    borrowed_descriptors()
    r, w = os.pipe()
    r2, w2 = os.pipe()
    p = select.poll()
    other = select.poll()
    p.register(r)
    p.modify(r, select.POLLIN)
    track(p, r2)
    assert len(p.poll(0)) == 0
    other.register(w, select.POLLOUT)
    assert len(other.poll(0)) == 1
    p.register(w, 0)
    assert len(p.poll(0)) == 0
    p.modify(w, select.POLLIN | select.POLLOUT)
    for fd, mask in p.poll():
        assert fd == w and mask & select.POLLOUT
    p.register(w, select.POLLIN)
    assert len(p.poll(0)) == 0
    p.unregister(w)
    assert len(other.poll(0)) == 1
    other.unregister(w)
    rr, ww, xx = select.select([r, r2], [w2, w, w2], [], 0)
    assert len(rr) == 0 and len(xx) == 0
    assert len(ww) == 3 and ww[0] == w2 and ww[1] == w and ww[2] == w2
    os.close(w)
    ready = p.poll(100)
    assert len(ready) == 1 and ready[0][0] == r and ready[0][1] & select.POLLHUP
    rr, ww, xx = select.select([r, r, r2], [], [], 0)
    assert len(rr) == 2 and rr[0] == r and rr[1] == r
    assert len(ww) == 0 and len(xx) == 0
    os.close(r)
    p.modify(r, 0)
    # An empty interest mask may produce no event on Darwin. Compare the
    # kernel's complete response with CPython on the same platform.
    print(observed_mask(p.poll(0), r))
    p.register(r)
    invalid = p.poll(0)
    assert len(invalid) == 1 and invalid[0][0] == r and invalid[0][1] == select.POLLNVAL
    try:
        select.select([r], [], [], 0)
    except OSError as e:
        assert e.errno == errno.EBADF
    else:
        raise AssertionError("closed fd in select")
    p.unregister(r)
    try:
        p.modify(r, select.POLLIN)
    except FileNotFoundError as e:
        assert e.errno == errno.ENOENT
    else:
        raise AssertionError("missing modify")
    try:
        p.unregister(r)
    except KeyError:
        pass
    else:
        raise AssertionError("missing unregister")
    os.dup2(w2, r)
    assert len(p.poll(0)) == 0
    p.register(r, select.POLLOUT)
    assert p.poll(0)[0][0] == r
    p.unregister(r)
    os.close(r)
    p.unregister(r2)
    os.close(r2)
    p.register(w2, 0)
    print(observed_mask(p.poll(0), w2))
    p.modify(w2, select.POLLOUT)
    print(observed_mask(p.poll(0), w2))
    p.unregister(w2)
    os.close(w2)
    print("state passed")


def bounds() -> None:
    p = select.poll()
    for mask in [-1, -(1 << 80)]:
        try:
            p.register(0, mask)
        except ValueError as e:
            assert str(e) == "Cannot convert negative int"
        else:
            raise AssertionError("negative mask")
    for mask in [65536, 1 << 80]:
        try:
            p.register(0, mask)
        except OverflowError as e:
            assert str(e) == "Python int too large for C unsigned short"
        else:
            raise AssertionError("overflowing mask")
    try:
        p.poll(1 << 80)
    except OverflowError as e:
        assert str(e) == "timestamp too large to convert to C PyTime_t"
    else:
        raise AssertionError("overflowing time")
    try:
        p.register(-1)
    except ValueError:
        pass
    else:
        raise AssertionError("negative fd")
    try:
        select.select([], [], [], -0.01)
    except ValueError:
        pass
    else:
        raise AssertionError("negative seconds")
    print("bounds passed")


def bridge() -> None:
    p = select.poll()
    i = 3
    while i < len(sys.argv):
        p.register(int(sys.argv[i]), int(sys.argv[i + 1]))
        i += 2
    timeout = sys.argv[2]
    if timeout == "None":
        ready = p.poll(None)
    elif timeout.startswith("float:"):
        ready = p.poll(float(timeout[6:]))
    else:
        ready = p.poll(int(timeout))
    for fd, mask in ready:
        print(fd)
        print(mask)


def bridge_select() -> None:
    fd = int(sys.argv[2])
    if sys.argv[3] == "None":
        rr, ww, xx = select.select([fd], [], [], None)
    else:
        rr, ww, xx = select.select([fd], [], [], int(sys.argv[3]))
    assert len(ww) == 0 and len(xx) == 0
    for result in rr:
        print(result)


def bridge_limit() -> None:
    p = select.poll()
    p.register(1)
    value = int(sys.argv[3])
    if sys.argv[2] == "register":
        p.register(0, value)
    elif sys.argv[2] == "modify":
        p.modify(1, value)
    else:
        p.poll(value)


def wait() -> None:
    p = select.poll()
    r = int(sys.argv[3])
    p.register(r, select.POLLIN)
    mode = sys.argv[2]
    if mode == "none":
        ready = p.poll(None)
    elif mode == "omitted":
        ready = p.poll()
    elif mode == "negative":
        ready = p.poll(-1e-100)
    elif mode == "fractional":
        ready = p.poll(35.25)
    else:
        rr, ww, xx = select.select([r], [], [], 0.03525)
        assert len(rr) == 0 and len(ww) == 0 and len(xx) == 0
        print("timeout")
        return
    if mode == "fractional":
        assert len(ready) == 0
        print("timeout")
    else:
        assert len(ready) == 1 and ready[0][0] == r and ready[0][1] & select.POLLIN
        print("awake")


if sys.argv[1] == "state":
    state()
elif sys.argv[1] == "bounds":
    bounds()
elif sys.argv[1] == "poll":
    bridge()
elif sys.argv[1] == "select":
    bridge_select()
elif sys.argv[1] == "limit":
    bridge_limit()
else:
    wait()
