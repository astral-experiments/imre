from contextlib import asynccontextmanager


def current():
    events = []

    class Steps:
        def __init__(self, values, suppress):
            self.values = list(values)
            self.suppress = suppress

        def __aiter__(self):
            return self

        async def __anext__(self):
            if not self.values:
                raise StopAsyncIteration
            return self.values.pop(0)

        async def athrow(self, error):
            events.append(error)
            if self.suppress:
                raise StopAsyncIteration
            raise error

        async def aclose(self):
            events.append("closed")

    @asynccontextmanager
    def manager(*values, suppress=False):
        return Steps(values, suppress)

    @manager("value")
    async def decorated(x):
        return x

    async def check():
        one = await decorated(1)
        two = await decorated(2)
        assert (one, two) == (1, 2)

        async with manager("opened") as resource:
            assert resource == "opened"

        failure = ValueError("body")
        async with manager("value", suppress=True):
            raise failure
        assert events == [failure]

        try:
            async with manager("value"):
                raise failure
        except ValueError as raised:
            assert raised is failure
        else:
            raise AssertionError("expected the original exception")
        assert events == [failure, failure]

        try:
            async with manager():
                raise AssertionError("unexpected entry")
        except RuntimeError as error:
            assert str(error) == "generator didn't yield"

        try:
            async with manager("first", "extra"):
                pass
        except RuntimeError as error:
            assert str(error) == "generator didn't stop"
        else:
            raise AssertionError("extra item was accepted")
        assert events == [failure, failure, "closed"]

    coroutine = check()
    try:
        coroutine.send(None)
    except StopIteration as finished:
        assert finished.value is None
    else:
        raise AssertionError("unexpected suspension")
