"""With binds its exit descriptor first and reports the missing special method."""
events = []


class Lookup:
    def __init__(self, name):
        self.name = name

    def __get__(self, instance, owner):
        events.append(self.name)
        return self.call

    def call(self, *args):
        events.append("call-" + self.name)


class Manager:
    __enter__ = Lookup("enter")
    __exit__ = Lookup("exit")


class ExitOnly:
    __exit__ = Lookup("unused-exit")


class AsyncProtocol:
    def __aenter__(self):
        raise AssertionError("suggesting async with must not call __aenter__")

    def __aexit__(self, *args):
        raise AssertionError("suggesting async with must not call __aexit__")


def missing(obj):
    try:
        with obj:
            raise AssertionError("invalid manager entered")
    except TypeError as exc:
        return str(exc)
    raise AssertionError("missing TypeError")


def current():
    events.clear()
    with Manager():
        events.append("body")
    assert events == ["exit", "enter", "call-enter", "body", "call-exit"]
    assert missing(object()) == "'object' object does not support the context manager protocol (missed __exit__ method)"
    assert missing(ExitOnly()) == (
        "'" + __name__ + ".ExitOnly' object does not support the context manager protocol (missed __enter__ method)"
    )
    assert missing(AsyncProtocol()).endswith(
        "(missed __exit__ method) but it supports the asynchronous context manager protocol. Did you mean to use 'async with'?"
    )
    print("context lookup verified")
