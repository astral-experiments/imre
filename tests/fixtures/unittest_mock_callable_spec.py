from unittest.mock import Mock, call, create_autospec, patch


def send(parcel, depot, *, priority=False):
    return f"{parcel} via {depot}"


class Dispatch:
    def __init__(self, submit):
        self.submit = submit

    def express(self, parcel):
        return self.submit(parcel, depot="west", priority=True)


desk = Dispatch(send)
with patch.object(desk, "submit", spec=True, return_value="queued") as submit:
    assert desk.express("P17") == "queued"
    assert submit.call_args == call("P17", depot="west", priority=True)
    submit.assert_called_once_with(parcel="P17", depot="west", priority=True)
    submit.assert_any_call("P17", "west", priority=True)
    submit.assert_has_calls([call("P17", "west", priority=True)])
    try:
        submit.assert_called_with("P17", lane="west")
    except AssertionError as error:
        assert isinstance(error.__cause__, TypeError)
    else:
        raise AssertionError("invalid expectation matched")
assert desk.submit is send
assert desk.express("P17") == "P17 via west"

audit = Mock(spec=send, return_value="received")
assert Dispatch(audit).express("P18") == "received"
audit.assert_called_with(parcel="P18", depot="west", priority=True)
# A plain spec accepts calls that cannot bind. Dropping its signature leaves
# the original argument spelling available for later assertions.
audit(unknown="P19")
assert audit.call_count == 2
audit.mock_add_spec(["send"])
audit.assert_called_with(unknown="P19")

for make in (Mock(spec=set), Mock(spec_set=frozenset)):
    make(["Ada"])
    make.assert_called_once_with(["Ada"])


class Adapter:
    def __init__(self):
        self.events = []
        self.reads = 0

    def __dir__(self):
        self.events.append("dir")
        return ["__call__", "route"]

    @property
    def route(self):
        self.events.append("route")
        return "west"

    def named(self, parcel):
        pass

    def positional(self, parcel, /):
        pass

    @property
    def __call__(self):
        self.events.append("call")
        self.reads += 1
        return self.positional if self.reads == 3 else self.named


adapter = Adapter()
submit = create_autospec(adapter)
assert adapter.events == ["dir", "call", "call", "dir", "call", "route"]
try:
    submit(parcel="P17")
except TypeError:
    pass
else:
    raise AssertionError("call-time signature accepted a keyword for a positional-only parameter")
submit.assert_not_called()
submit("P17")
submit.assert_called_once_with(parcel="P17")


class UnavailableInit:
    def __get__(self, obj, owner):
        raise problem


class Guarded(set):
    __init__ = UnavailableInit()


problem = AttributeError("initializer unavailable")
try:
    Mock(spec=Guarded)
except AttributeError as error:
    assert error is problem
else:
    raise AssertionError("user initializer error was suppressed")


class ClassProxy:
    @property
    def __class__(self):
        return type

    def __dir__(self):
        return []


proxy = Mock(spec=ClassProxy())
proxy()
proxy.assert_called_once_with()


class EnumeratedProperty(property):
    def __dir__(self):
        raise directory_error


directory_error = NotImplementedError("no directory")
try:
    create_autospec(EnumeratedProperty(lambda self: 0))
except NotImplementedError as error:
    assert error is directory_error
else:
    raise AssertionError("the property subclass's directory error was suppressed")
assert not callable(create_autospec(None))


class GuardedDirectory(property):
    def __getattribute__(self, key):
        if key == "__dict__":
            raise directory_error
        return object.__getattribute__(self, key)


try:
    create_autospec(GuardedDirectory(lambda self: 0))
except NotImplementedError as error:
    assert error is directory_error
else:
    raise AssertionError("default directory lookup suppressed a descriptor exception")


mro_reads = []


class Meta(type):
    def __dir__(cls):
        return ["__call__"]

    @property
    def __mro__(cls):
        mro_reads.append(cls)
        return cls, object


class Plugin(metaclass=Meta):
    def __call__(self, parcel):
        pass


instance = create_autospec(Plugin, instance=True)
instance("P17")
assert instance.call_count == 1 and mro_reads == [Plugin]
