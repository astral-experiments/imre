"""Exercise boxed islice acquisition, selection and exhaustion."""

from itertools import islice


class Source:
    def __init__(self):
        self.events = []
        self.position = 0

    def __iter__(self):
        self.events.append("iter")
        return self

    def __next__(self):
        value = self.position
        self.position += 1
        self.events.append(value)
        return value


source = Source()
view = islice(source, 1, 6, 2)
assert source.events == ["iter"]
assert iter(view) is view
assert next(view) == 1
view.__init__([], 0)
assert source.events == ["iter", 0, 1]
assert list(view) == [3, 5]
assert source.events == ["iter", 0, 1, 2, 3, 4, 5]
assert list(view) == []
assert next(source) == 6
assert list(islice(source, 4, 2)) == []
assert next(source) == 11


class SliceSubclass(islice):
    pass


view = SliceSubclass([10, 20, 30], 2)
assert next(view) == 10
view.__init__([40], 1)
assert list(view) == [20]


class Init:
    def __get__(self, instance, owner):
        assert instance is not None
        return instance.initialize


class WithInit(islice):
    __init__ = Init()

    def initialize(self, *args, label):
        self.label = label


class Stop:
    def __index__(self):
        source.events.append("index")
        return 1


source = Source()
view = WithInit(source, Stop(), label=3)
assert source.events == ["index", "iter"]
assert view.label == 3 and list(view) == [0]
assert source.events == ["index", "iter", 0]


class BadIndex:
    def __index__(self):
        raise RuntimeError("bad index")


source = Source()
try:
    islice(source, BadIndex(), -1, 0)
except ValueError as error:
    assert str(error).startswith("Stop argument for islice()")
else:
    raise AssertionError("invalid stop accepted")
assert source.events == []
for args, keywords, message in [
    ((), {}, "islice expected at least 2 arguments, got 0"),
    ((source, 1, 2, 3, 4), {}, "islice expected at most 4 arguments, got 5"),
    ((), {"start": 1}, "islice() takes no keyword arguments"),
]:
    try:
        islice(*args, **keywords)
    except TypeError as error:
        assert str(error) == message
    else:
        raise AssertionError("invalid call accepted")
assert source.events == []


class Fails(Source):
    def __next__(self):
        if self.position == 1:
            raise RuntimeError("stopped")
        return super().__next__()


view = islice(Fails(), 2, None)
try:
    next(view)
except RuntimeError as error:
    assert str(error) == "stopped"
else:
    raise AssertionError("source error was lost")
assert next(view, "done") == "done"
print("passed")
