import errno
import os
import select


def current():
    read, write = os.pipe()
    poll = select.poll()
    try:
        assert select.select([read], [write, write], [], 0) == ([], [write, write], [])
        poll.register(write, select.POLLIN)
        assert poll.poll(0) == []
        poll.register(write, select.POLLOUT)
        assert poll.poll(None) == [(write, select.POLLOUT)]
        poll.modify(write, 0)
        poll.register(read, 0)
        assert poll.poll(0) == []
    finally:
        os.close(write)
    try:
        assert select.select([read, read], [], [], 0) == ([read, read], [], [])
        assert dict(poll.poll(0)) == {write: select.POLLNVAL, read: select.POLLHUP}
        try:
            select.select([write], [], [], 0)
        except OSError as error:
            assert error.errno == errno.EBADF
        else:
            raise AssertionError("select accepted a closed descriptor")
        poll.unregister(write)
        poll.unregister(read)
        assert poll.poll(0) == []
    finally:
        os.close(read)
    print("selected twice and polled hangup")
