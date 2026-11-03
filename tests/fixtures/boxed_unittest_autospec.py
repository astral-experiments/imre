from inspect import signature
from unittest import TestCase
from unittest.mock import NonCallableMagicMock, create_autospec, seal


class Version:
    def __get__(self, instance, owner):
        return 7

    def __set__(self, instance, value):
        raise AttributeError


class Client:
    version = Version()

    def __init__(self, host):
        pass

    def send(self, value, /, *, retry=False):
        pass

    class Codec:
        @staticmethod
        def encode(value):
            pass

        @classmethod
        def decode(cls, value):
            pass


check = TestCase()
factory = create_autospec(Client, spec_set=True)
with check.assertRaises(TypeError):
    factory()
factory.assert_not_called()
instance = factory("localhost")
assert isinstance(instance, NonCallableMagicMock) and not callable(instance)
with check.assertRaises(AttributeError):
    instance.extra = 1

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

seal(client)
assert client.extra == 1 and client.Codec.encode("more") == "encoded"
with check.assertRaises(AttributeError):
    client.Codec.missing = 1
print("autospec checked")
