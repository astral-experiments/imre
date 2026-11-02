from selectors import EVENT_READ, EVENT_WRITE, PollSelector, SelectSelector
import selectors
import os


def current():
    assert SelectSelector is selectors.SelectSelector
    assert PollSelector is selectors.PollSelector
    assert SelectSelector is not PollSelector
    for factory, other in ((SelectSelector, PollSelector), (PollSelector, SelectSelector)):
        with factory() as selector:
            assert type(selector) is factory
            assert isinstance(selector, factory)
            assert not isinstance(selector, other)
            with open("selector-ready.bin", "w+b", 0) as stream:
                key = selector.register(stream, EVENT_READ)
                ready = selector.select(0)
                assert len(ready) == 1 and ready[0][0] is key and ready[0][1] == EVENT_READ
                both = selector.modify(stream, EVENT_READ | EVENT_WRITE)
                ready = selector.select(0)
                assert len(ready) == 1 and ready[0][0] is both and ready[0][1] == 3
                fd = stream.fileno()
            if factory is SelectSelector:
                try:
                    selector.select(0)
                except OSError as exc:
                    assert exc.errno == 9
                else:
                    raise AssertionError("select accepted a closed descriptor")
            else:
                ready = selector.select(0)
                assert len(ready) == 1 and ready[0][0] is both and ready[0][1] == 3
            assert selector.unregister(fd) is both
    r, w = os.pipe()
    try:
        with SelectSelector() as selector:
            reader = selector.register(r, EVENT_READ)
            assert selector.select(-1) == []
            assert selector.select(0.005) == []
            writer = selector.register(w, EVENT_WRITE)
            ready = selector.select(0)
            assert len(ready) == 1 and ready[0][0] is writer and ready[0][1] == EVENT_WRITE
            assert selector.unregister(w) is writer
            os.write(w, b"x")
            ready = selector.select(None)
            assert len(ready) == 1 and ready[0][0] is reader and ready[0][1] == EVENT_READ
            assert os.read(r, 1) == b"x" and selector.select(0) == []
            for invalid in (0, 1 << 100):
                try:
                    selector.modify(r, invalid)
                except ValueError:
                    pass
                else:
                    raise AssertionError("modify accepted invalid events")
                assert selector.register(r, EVENT_READ).fd == r
    finally:
        os.close(r)
        os.close(w)
    with SelectSelector() as selector:
        for fd, error in ((2**20, ValueError), (2**80, OverflowError)):
            key = selector.register(fd, EVENT_READ)
            assert key.fd == fd and selector.get_key(fd) is key
            try:
                selector.select(0)
            except error:
                pass
            else:
                raise AssertionError("select accepted an out-of-range descriptor")
            assert selector.unregister(fd) is key
    print("select and poll preserved distinct raw descriptor semantics")
