import inspect


class Manager:
    def __init__(self):
        self.events = []

    async def __aenter__(self) -> int:
        self.events.append("enter")
        return 42

    async def __aexit__(self, etype, value, tb):
        assert etype is value is tb is None
        self.events.append("exit")


async def read_value(manager) -> int:
    async with manager as value:
        return value


manager = Manager()
pending = read_value(manager)
assert inspect.iscoroutinefunction(read_value)
assert pending.cr_code is read_value.__code__
assert manager.events == []
try:
    pending.send(None)
except StopIteration as done:
    assert done.value == 42
else:
    raise AssertionError("async with did not complete on the first send")
assert manager.events == ["enter", "exit"]
