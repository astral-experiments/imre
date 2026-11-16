import os
import select


def current(mask):
    if mask:
        select.epoll = None
    else:
        del select.epoll
    import selectors

    read, write = os.pipe()
    try:
        with selectors.PollSelector() as selector:
            key = selector.register(read, selectors.EVENT_READ)
            os.write(write, b"x")
            assert selector.select(0.2) == [(key, selectors.EVENT_READ)]
    finally:
        os.close(read)
        os.close(write)
