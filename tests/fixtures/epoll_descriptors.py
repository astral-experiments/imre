import errno
import os
import select
from select import EPOLLIN, epoll
from selectors import EVENT_READ, EpollSelector

assert vars(select)["epoll"] is epoll
assert vars(select)["EPOLLIN"] == EPOLLIN

r, w = os.pipe()
try:
    with epoll() as first:
        second = epoll.fromfd(os.dup(first.fileno()))
        with second:
            first.register(r, EPOLLIN)
            os.write(w, b'x')
            assert second.poll(0, 1) == [(r, EPOLLIN)]
        assert first.poll(0, 1) == [(r, EPOLLIN)]
        first.unregister(r)
        assert first.poll(0, 1) == []
        if hasattr(select, "EPOLLEXCLUSIVE"):
            first.register(r, EPOLLIN | select.EPOLLEXCLUSIVE)
            assert first.poll(0, 1) == [(r, EPOLLIN)]
            first.unregister(r)
            print("exclusive pipe ready")
    with EpollSelector() as selector:
        key = selector.register(r, EVENT_READ)
        assert selector.select(0) == [(key, EVENT_READ)]
        assert selector.unregister(r) is key
        with open(__file__, 'rb') as file:
            try:
                selector.register(file, EVENT_READ)
            except PermissionError as error:
                assert error.errno == errno.EPERM
            else:
                raise AssertionError("epoll accepted a regular file")
            assert len(selector.get_map()) == 0
    assert os.read(r, 1) == b'x'
finally:
    os.close(r)
    os.close(w)
