from selectors import BaseSelector, EVENT_READ, EVENT_WRITE, PollSelector, SelectSelector
import selectors
from collections.abc import Mapping
import os


class Descriptor(int):
    pass


class Missing:
    def fileno(self):
        return 999999

    def __repr__(self):
        raise LookupError("custom repr")


def current():
    class Wrapper(BaseSelector):
        def __init__(self, backend):
            self.backend = backend

        def register(self, fileobj, events, data=None):
            return self.backend.register(fileobj, events, data)

        def unregister(self, fileobj):
            return self.backend.unregister(fileobj)

        def select(self, timeout=None):
            return self.backend.select(timeout)

        def get_map(self):
            return self.backend.get_map()

    try:
        BaseSelector()
    except TypeError:
        pass
    else:
        raise AssertionError("abstract selector accepted")
    assert SelectSelector is selectors.SelectSelector
    assert PollSelector is selectors.PollSelector
    assert SelectSelector is not PollSelector
    for factory, other in ((SelectSelector, PollSelector), (PollSelector, SelectSelector)):
        with factory() as selector:
            assert issubclass(factory, BaseSelector) and isinstance(selector, BaseSelector)
            with Wrapper(selector) as wrapper:
                rd, wr = os.pipe()
                try:
                    key = wrapper.register(rd, EVENT_READ, "first")
                    assert wrapper.get_key(rd) is key
                    key = wrapper.modify(rd, EVENT_READ, "second")
                    os.write(wr, b"x")
                    assert wrapper.select(0) == [(key, EVENT_READ)]
                    assert wrapper.unregister(rd) is key
                finally:
                    os.close(rd)
                    os.close(wr)
            assert type(selector) is factory
            assert isinstance(selector, factory)
            assert not isinstance(selector, other)
            mapping = selector.get_map()
            assert mapping is selector.get_map() and isinstance(mapping, Mapping)
            assert not mapping
            with open("selector-ready.bin", "w+b", 0) as stream:
                key = selector.register(stream, EVENT_READ)
                assert mapping.get(stream) is mapping[Descriptor(stream.fileno())] is key
                missing = Missing()
                assert mapping.get(missing) is None
                try:
                    mapping[missing]
                except LookupError as exc:
                    assert exc.args == ("custom repr",)
                else:
                    raise AssertionError("missing key bypassed repr")
                assert list(mapping) == [stream.fileno()]
                assert list(mapping.values()) == [key]
                ready = selector.select(0)
                assert len(ready) == 1 and ready[0][0] is key and ready[0][1] == EVENT_READ
                both = selector.modify(stream, EVENT_READ | EVENT_WRITE)
                ready = selector.select(0)
                assert len(ready) == 1 and ready[0][0] is both and ready[0][1] == 3
                assert mapping[stream] is both
                try:
                    del mapping[stream]
                except TypeError:
                    pass
                else:
                    raise AssertionError("selector mapping allowed deletion")
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
            assert mapping[stream] is both
            assert selector.unregister(fd) is both
            selector.register(fd, EVENT_READ)
            iterator = iter(mapping)
        assert selector.get_map() is None and not mapping
        try:
            next(iterator)
        except RuntimeError:
            pass
        else:
            raise AssertionError("descriptor iterator ignored close")
        assert mapping.get(fd) is None
        try:
            selector.get_key(fd)
        except RuntimeError:
            pass
        else:
            raise AssertionError("closed selector returned a key")
        new_key = selector.register(fd, EVENT_READ)
        assert mapping[fd] is new_key and selector.get_map() is None
        selector.unregister(fd)
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
