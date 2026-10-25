"""Keep one native selector and its inherited descriptors alive for the host test."""
import errno
import os
import select
import selectors
import sys


if sys.argv[1] == "PollSelector":
    selector = selectors.PollSelector()
else:
    selector = selectors.DefaultSelector()
while True:
    action = input()
    a, b = int(input()), int(input())
    try:
        if action == "stop":
            selector.close()
            print("ok")
            break
        elif action == "register":
            selector.register(a, b)
        elif action == "unregister":
            selector.unregister(a)
        elif action == "close":
            os.close(a)
        elif action == "dup2":
            assert os.dup2(a, b) == b
        elif action == "before":
            # The original pair is idle; only the writable end has an event.
            ready = selector.select(0)
            assert len(ready) == 1 and ready[0][0].fd == a and ready[0][1] == selectors.EVENT_WRITE
        elif action == "after":
            # The replacement pair has a byte queued at the original fd number.
            r, w, x = select.select([a], [a, b], [], 0)
            assert len(r) == 1 and r[0] == a
            assert len(w) == 2 and w[0] == a and w[1] == b and len(x) == 0
            assert len(selector.select(0)) == 0
        else:
            raise AssertionError("unknown operation")
        print("ok")
    except KeyError:
        print("key")
    except OSError as error:
        assert error.errno == errno.EBADF
        print("badfd")
