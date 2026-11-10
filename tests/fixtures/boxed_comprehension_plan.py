"""Observable order, scope, and suspension at comprehension boundaries."""


def synchronous():
    events = []

    def record(label, value):
        events.append((label, value))
        return value

    x = "outside"
    mapping = {
        record("key", x): record("value", y)
        for x in record("outer", [0, 1, 2])
        if record("keep", x)
        for y in record("inner", [x, x + 1])
        if record("odd", y % 2)
    }
    print("dict", mapping, x, events)

    callbacks = [lambda: x for x in (1, 2, 3)]
    print("closure", [call() for call in callbacks], x)
    last = 0
    saved = [lambda n=(last := v): n for v in (2, 4)]
    assert (last, [callback() for callback in saved]) == (4, [2, 4])

    templates = [t"{x}" for x in ("one", "two")]
    assert [template.interpolations[0].value for template in templates] == ["one", "two"]

    print("set", sorted({x for x, y in [(2, 0), (1, 1), (2, 2)] if y}))

    events.clear()
    gen = (record("item", y) for x in record("outer", [4, 5])
           for y in record("inner", [x, x + 1]))
    print("created", events)
    print("first", next(gen), events)
    gen.close()
    print("closed", next(gen, "done"), events)

    def boom():
        raise LookupError("boom")

    try:
        gen = (x for x in boom())
    except LookupError as exc:
        print("outer error", exc.args)
    gen = (x for x in [1] for y in boom())
    try:
        next(gen)
    except LookupError as exc:
        print("inner error", exc.args, next(gen, "done"))

    last = "initial"
    gen = ((last := x) for x in [6, 7])
    print("walrus", last, next(gen), last, next(gen), last)


class Pause:
    def __init__(self, label, result):
        self.label = label
        self.result = result

    def __await__(self):
        yield self.label
        return self.result


async def numbers():
    yield 1
    yield 2


def async_items():
    return (await Pause(("item", y), y) for x in [3, 4]
            async for y in numbers() if x == 3)


def async_outer():
    return (x async for x in numbers())


async def mixed():
    x = "enclosing"
    mapping = {
        await Pause(("key", x), x): await Pause(("value", y), y)
        async for x in numbers()
        if await Pause(("filter", x), x == 2)
        for y in [x, x + 1]
    }
    print("async dict", mapping, x)

    gen = async_items()
    print("async first", await anext(gen))
    await gen.aclose()
    print("async closed", await anext(gen, "done"))
    print("async outer", [x async for x in async_outer()])

    class Target:
        def __setitem__(self, key, value):
            raise StopAsyncIteration("target failed")

    target = Target()
    try:
        [x async for target[0] in numbers()]
    except StopAsyncIteration as exc:
        print("async target error", exc.args)

    def reject(value):
        raise StopAsyncIteration("filter failed")

    try:
        [x async for x in numbers() if reject(x)]
    except StopAsyncIteration as exc:
        print("async filter error", exc.args)


def suspension():
    coro = mixed()
    while True:
        try:
            print("pause", coro.send(None))
        except StopIteration:
            break


def current():
    synchronous()
    suspension()
