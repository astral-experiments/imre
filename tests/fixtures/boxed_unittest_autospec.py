from dataclasses import dataclass
from inspect import signature
from time import ctime
from types import MethodType
from unittest import TestCase
from unittest.mock import InvalidSpecError, NonCallableMagicMock, call, create_autospec, seal


class Version:
    def __get__(self, instance, owner):
        return 7

    def __set__(self, instance, value):
        raise AttributeError


class Sink:
    def write(self, value):
        pass


class Client:
    version = Version()
    sink = Sink()

    def __init__(self, host):
        self.host = host

    def send(self, value, /, *, retry=False):
        pass

    class Codec:
        @staticmethod
        def encode(value):
            pass

        @classmethod
        def decode(cls, value):
            pass


class Queue(list):
    def publish(self, value):
        pass


check = TestCase()
namespace = list.__dict__
assert namespace["append"] is list.append and "__call__" not in namespace
assert namespace["__len__"] is list.__len__ and namespace["__hash__"] is None
with check.assertRaises(TypeError):
    namespace["append"] = None
queue = create_autospec(Queue, spec_set=True, instance=True)
assert not callable(queue)
queue.append("first")
queue.append.assert_called_once_with("first")
with check.assertRaises(TypeError):
    queue.publish()
for method in (list.append, [].append, set.add, set().add):
    child = create_autospec(method)
    child()
    child(1, 2)
    assert child.call_args_list == [call(), call(1, 2)]


class Enqueue(Queue):
    __call__ = list.append


class Disabled(Enqueue):
    __call__ = None


enqueue = create_autospec(Enqueue, instance=True)
enqueue("first")
assert enqueue.call_args == call("first")
with check.assertRaises(TypeError):
    enqueue()
with check.assertRaises(AssertionError):
    enqueue.assert_called_once_with("first")
with check.assertRaisesRegex(TypeError, "None is not a callable object"):
    create_autospec(Disabled, instance=True)
with check.assertRaisesRegex(TypeError, "None is not a callable object"):
    create_autospec(Disabled())
with check.assertRaisesRegex(TypeError, "the first argument must be callable"):
    create_autospec(Disabled)

factory = create_autospec(Client, spec_set=True)
with check.assertRaises(TypeError):
    factory()
factory.assert_not_called()
instance = factory("localhost")
assert isinstance(instance, NonCallableMagicMock) and not callable(instance)
with check.assertRaises(AttributeError):
    instance.extra = 1

live = create_autospec(Client("localhost"), spec_set=True)
assert isinstance(live, Client) and not callable(live)
assert isinstance(live.host, str)
with check.assertRaises(AttributeError):
    live.extra = 1
live.send("payload", retry=True)
with check.assertRaises(TypeError):
    live.send(value="payload")
live.send.assert_called_once_with("payload", retry=True)
with check.assertRaises(InvalidSpecError):
    create_autospec(live)

client = create_autospec(Client, instance=True)
assert isinstance(client, NonCallableMagicMock) and not callable(client)
with check.assertRaises(TypeError):
    client()
client.extra = 1
assert isinstance(client.version, NonCallableMagicMock)
assert isinstance(client.version, int) and not callable(client.version)
# Method signatures come from the class as it was when the mock was created.
Client.send = lambda self: None
assert str(signature(client.send)) == "(value, /, *, retry=False)"
client.send.return_value = "sent"
with check.assertRaises(TypeError):
    client.send(value=1)
client.send.assert_not_called()
assert client.send(1, retry=True) == "sent"
client.send.assert_called_once_with(1, retry=True)

for method in (client.Codec.encode, client.Codec.decode):
    with check.assertRaises(TypeError):
        method()
    method.assert_not_called()
    method.return_value = "encoded"
    assert method("data") == "encoded"

for name in ("encode", "decode"):
    descriptor = Client.Codec.__dict__[name]
    descriptor.label = name
    assert ("__call__" in dir(descriptor)) == isinstance(descriptor, staticmethod)
    method = create_autospec(descriptor, spec_set=True)
    method.label = "updated"
    with check.assertRaises(AttributeError):
        method.__code__ = None
    assert method.__class__ is type(descriptor)
    assert signature(method) == signature(getattr(Client.Codec, name))
    with check.assertRaises(TypeError):
        method()
    with check.assertRaises(TypeError):
        method(None, "data")
    method.assert_not_called()
    method("data")
    method.assert_called_once_with(value="data")

seal(client)
assert client.extra == 1 and client.Codec.encode("more") == "encoded"
with check.assertRaises(AttributeError):
    client.Codec.missing = 1
write = instance.sink.write
write("one")
write.assert_called_once_with(value="one")
write(value="two")
write.assert_called_with("two")
write.assert_any_call(value="one")
write.assert_has_calls([call(value="one"), call("two")])
instance.assert_has_calls([call.sink.write(value="one"), call.sink.write("two")])
assert write.call_args_list == [call("one"), call(value="two")]
with check.assertRaises(AssertionError):
    write.assert_has_calls([call("two"), call("one")])
with check.assertRaises(AssertionError) as invalid:
    write.assert_called_with(unknown="two")
assert isinstance(invalid.exception.__cause__, TypeError)

source = Client("localhost")
source.receive = lambda data: None
receive = create_autospec(source).receive
with check.assertRaises(TypeError):
    receive()
receive("packet")
receive.assert_called_once_with(data="packet")


@dataclass
class Request:
    sink: Sink
    count: int = 0


request_type = create_autospec(Request, spec_set=True)
with check.assertRaises(AttributeError):
    request_type.sink
for scalar in (bool, int, float, complex, str, bytes):
    assert scalar.__init__ is object.__init__
assert dict.__init__ is not object.__init__
request = request_type(Sink())
assert isinstance(request.count, int)
assert isinstance(request.sink, Sink) and isinstance(request.sink(), Sink)
request.sink.write("body")
request.sink.write.assert_called_once_with(value="body")
with check.assertRaises(AttributeError):
    request.sink.extra = 1

bound = MethodType(ctime, 0)
opaque = create_autospec(bound)
opaque(1, 2)
opaque.assert_called_once_with(1, 2)

sealed = create_autospec(bound)
seal(sealed)
with check.assertRaises(AttributeError):
    sealed()

print("autospec checked")
