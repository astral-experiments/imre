import os
import selectors
import signal


def current():
    read_fd, write_fd = os.pipe()
    try:
        with selectors.PollSelector() as selector:
            selector.register(read_fd, selectors.EVENT_READ, "before")

            def on_alarm(signum, frame):
                selector.modify(read_fd, selectors.EVENT_READ, "after")
                os.write(write_fd, b"x")

            previous = signal.signal(signal.SIGALRM, on_alarm)
            try:
                signal.alarm(1)
                ready = selector.select(5)
            finally:
                signal.alarm(0)
                signal.signal(signal.SIGALRM, previous)

            key = selector.get_key(read_fd)
            assert ready == [(key, selectors.EVENT_READ)]
            assert key.data == "after"
            assert os.read(read_fd, 1) == b"x"
    finally:
        os.close(read_fd)
        os.close(write_fd)
