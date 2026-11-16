"""A source frame retains the failed operation's byte range across suspension."""


def current():
    import traceback

    source = """\
error = LookupError("sentinel")
cleanup = []
class Pause:
    def __init__(self, value, fail=False):
        self.value = value
        self.fail = fail

    def __await__(self):
        try:
            yield self.value
            if self.fail:
                raise error
            return self.value
        finally:
            cleanup.append("leave")

class Box:
    def __getitem__(self, key):
        raise error

async def receiver():
    return (await Pause(3, True))[await Pause(1)]

async def subscription():
    return (
        await Pause(Box())
    )[
        await Pause(0)
    ]

async def not_awaitable():
    return await 123

"""
    scope = {}
    exec(compile(source, "suspended_positions.py", "exec"), scope)

    def run(name, pauses, expected, error_type, injected=None):
        task = scope[name]()
        try:
            for _ in range(pauses):
                task.send(None)
            if injected is None:
                task.send(None)
            else:
                task.throw(injected)
        except error_type as caught:
            if injected is not None:
                assert caught is injected
            elif error_type is LookupError:
                assert caught is scope["error"]
            frame = next(frame for frame in traceback.extract_tb(caught.__traceback__)
                         if frame.name == name)
            actual = (frame.lineno, frame.end_lineno, frame.colno, frame.end_colno)
            assert actual == expected, (actual, expected)
        else:
            raise AssertionError("missing exception")

    lines = source.splitlines()
    left = next(i for i, line in enumerate(lines, 1) if "return (await Pause" in line)
    run("receiver", 1, (left, left, 12, 32), LookupError)
    assert scope["cleanup"] == ["leave"]
    run("receiver", 1, (left, left, 12, 32), ValueError, ValueError("injected"))
    assert scope["cleanup"] == ["leave", "leave"]
    sub = lines.index("    return (") + 1
    run("subscription", 2, (sub, sub + 4, 11, 5), LookupError)
    assert scope["cleanup"] == ["leave"] * 4
    bad = lines.index("    return await 123") + 1
    run("not_awaitable", 0, (bad, bad, 11, 20), TypeError)

    saved_method_call()
    failing_test_operand()


def saved_method_call():
    import traceback

    marker = LookupError("failure")
    calls = []

    class Service:
        @property
        def callback(self):
            calls.append("lookup")
            if fail == "lookup":
                raise marker
            return self.reject

        def reject(self, first, second):
            calls.append((first, second))
            raise marker

    class Gate:
        def __await__(self):
            return (yield "ready")

    def argument():
        calls.append("argument")
        if fail == "argument":
            raise marker
        return 2

    async def invoke(service):
        return service.callback(
            await Gate(),
            argument(),
        )

    start = invoke.__code__.co_firstlineno + 1
    for fail in ("call", "argument", "lookup"):
        calls.clear()
        coro = invoke(Service())
        try:
            assert coro.send(None) == "ready"
            coro.send(1)
        except LookupError as error:
            assert error is marker
            frame = next(f for f in traceback.extract_tb(error.__traceback__) if f.name == "invoke")
            assert (frame.lineno, frame.end_lineno, frame.colno, frame.end_colno) == {
                "call": (start, start + 3, 15, 9),
                "argument": (start + 2, start + 2, 12, 22),
                "lookup": (start, start, 15, 31),
            }[fail]
        assert calls == {
            "call": ["lookup", "argument", (1, 2)],
            "argument": ["lookup", "argument"],
            "lookup": ["lookup"],
        }[fail]

def failing_test_operand():
    import traceback

    class Bad:
        def __bool__(self):
            raise LookupError("test")

    def choose(value):
        return 1 if value else 2

    try:
        choose(Bad())
    except LookupError as error:
        frame = traceback.extract_tb(error.__traceback__)[-2]
        assert frame.name == "choose"
        if frame.colno is not None:
            assert (frame.colno, frame.end_colno) == (20, 25)
