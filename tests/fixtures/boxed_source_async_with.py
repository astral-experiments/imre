class Pause:
    def __init__(self, label):
        self.label = label
        self.started = False

    def __await__(self):
        return self

    def __iter__(self):
        return self

    def __next__(self):
        if self.started:
            raise StopIteration
        self.started = True
        return self.label

    def send(self, value):
        assert self.started
        return self.__next__()


def current():
    scope = {"Pause": Pause}
    exec("""\
class Manager:
    def __init__(self, fail_at, suppress):
        self.fail_at = fail_at
        self.suppress = suppress
        self.body_error = ValueError("body")
        self.enter_error = ValueError("enter")
        self.exit_error = ValueError("exit")
        self.seen = []

    async def acquire(self):
        await Pause("acquire")
        return self

    async def __aenter__(self):
        await Pause("enter")
        if self.fail_at == "enter":
            raise self.enter_error
        return self

    async def body(self):
        await Pause("body")
        if self.fail_at in ("body", "exit"):
            raise self.body_error

    async def __aexit__(self, typ, value, tb):
        if self.fail_at in ("body", "exit"):
            assert typ is ValueError and value is self.body_error
            assert tb is not None and tb is value.__traceback__
        else:
            assert (typ, value, tb) == (None, None, None)
        self.seen.append("exit")
        await Pause("exit")
        if self.fail_at == "exit":
            raise self.exit_error
        return self.suppress


async def run(manager):
    async with await manager.acquire() as value:
        assert value is manager
        await value.body()
    return 42
""", scope)
    for fail_at, suppress in (("", False), ("enter", False), ("body", False),
                              ("body", True), ("exit", False)):
        manager = scope["Manager"](fail_at, suppress)
        coro = scope["run"](manager)
        steps = []
        try:
            while True:
                steps.append(coro.send(None))
        except StopIteration as done:
            assert done.value == 42 and (not fail_at or suppress)
        except ValueError as error:
            if fail_at == "enter":
                assert error is manager.enter_error
            elif fail_at == "body":
                assert not suppress and error is manager.body_error
            else:
                assert fail_at == "exit" and error is manager.exit_error
        if fail_at == "enter":
            assert steps == ["acquire", "enter"] and manager.seen == []
        else:
            assert steps == ["acquire", "enter", "body", "exit"] and manager.seen == ["exit"]
