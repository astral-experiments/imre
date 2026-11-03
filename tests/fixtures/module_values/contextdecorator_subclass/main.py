import contextlib

events = []


class Mark(contextlib.ContextDecorator):
    def __enter__(self):
        events.append("enter")

    def __exit__(self, typ, value, tb):
        events.append(typ)


class Client:
    @Mark()
    def total(self, value):
        events.append(self)
        return value + 1


client = Client()
assert client.total(2) == 3
assert events == ["enter", client, None]
assert Client.total.__name__ == "total"
