import types
import weakref
from inspect import getcoroutinelocals


class Pause:
    def __await__(self):
        result = yield "paused"
        return result


def current():
    events = []
    error = ValueError("cleanup")
    scope = {"events": events, "pause": Pause(), "error": error}
    exec("""\
class Manager:
    async def __aenter__(self):
        events.append("enter")
        return await pause

    async def __aexit__(self, *exc_info):
        events.append("exit")
        raise error


def suspend(value):
    return (yield value)


async def forward(awaitable):
    return await awaitable
""", scope)
    enter_method = scope["Manager"].__aenter__
    assert types.coroutine(enter_method) is enter_method
    manager = scope["Manager"]()
    enter = manager.__aenter__()
    assert events == []
    assert enter.send(None) == "paused"
    assert events == ["enter"]
    token = object()
    try:
        enter.send(token)
    except StopIteration as done:
        assert done.value is token
    else:
        raise AssertionError("coroutine did not return")
    try:
        enter.send(None)
    except RuntimeError as caught:
        assert "reuse" in str(caught)
    else:
        raise AssertionError("coroutine allowed reuse")

    leave = manager.__aexit__(None, None, None)
    assert events == ["enter"]
    try:
        leave.send(None)
    except ValueError as caught:
        assert caught is error
    else:
        raise AssertionError("cleanup exception was lost")
    assert events == ["enter", "exit"]

    suspend = scope["suspend"]
    forward = scope["forward"]
    old = suspend(token)
    original_code = suspend.__code__
    proxy = weakref.proxy(suspend)
    assert types.coroutine(proxy) is proxy
    marked_code = suspend.__code__
    assert old.gi_code is original_code and marked_code is not original_code
    assert types.coroutine(suspend) is suspend and suspend.__code__ is marked_code

    fresh = suspend(token)
    assert fresh.gi_code is marked_code

    class Factory:
        def __call__(self, value, *, expected):
            assert value is expected
            return value

    factory = Factory()
    decorated = types.coroutine(factory)
    assert decorated.__wrapped__ is factory
    assert decorated(fresh, expected=fresh) is fresh
    wrapped = decorated(old, expected=old)
    assert isinstance(wrapped, types._GeneratorWrapper)
    assert wrapped.__await__() is old
    assert decorated(token, expected=token) is token

    class Impostor:
        __class__ = types.GeneratorType

        @property
        def gi_code(self):
            raise error

    impostor = Impostor()
    try:
        decorated(impostor, expected=impostor)
    except ValueError as caught:
        assert caught is error
    else:
        raise AssertionError("factory result lost gi_code exception")

    relay = forward(old)
    relay_locals = getcoroutinelocals(relay)
    assert relay_locals["awaitable"] is old
    relay_locals["awaitable"] = fresh
    assert relay.send(None) is token
    result = object()
    try:
        relay.send(result)
    except StopIteration as done:
        assert done.value is result
    else:
        raise AssertionError("decorated generator did not return")
    assert getcoroutinelocals(relay) == {}
    assert relay_locals["awaitable"] is fresh
    try:
        forward(old).send(None)
    except TypeError:
        pass
    else:
        raise AssertionError("decoration made an existing generator awaitable")
    finally:
        old.close()

    awaited_call_arguments()
    awaited_binary_operands()
    awaited_comparisons()


def awaited_call_arguments():
    events = []

    def mark(value):
        events.append(value)
        return value

    class Receiver:
        def collect(self, left, first, middle, *, second, last):
            events.append('call')
            return left, first, middle, second, last

    receiver = Receiver()

    def get_receiver():
        events.append('receiver')
        return receiver

    scope = {'get_receiver': get_receiver, 'mark': mark, 'pause': Pause()}
    exec("""\
async def ordered():
    return get_receiver().collect(mark('left'), await pause, mark('middle'),
                                  second=await pause, last=mark('right'))

async def first():
    try:
        mark(await pause)
    finally:
        mark('finally')

async def forward(value):
    return value

async def outer():
    return await forward(await pause)
""", scope)

    coro = scope['ordered']()
    assert coro.send(None) == 'paused'
    assert events == ['receiver', 'left']
    # The original bound method must survive until both arguments have resumed.
    receiver.collect = None
    assert coro.send(1) == 'paused'
    assert events == ['receiver', 'left', 'middle']
    try:
        coro.send(2)
    except StopIteration as done:
        assert done.value == ('left', 1, 'middle', 2, 'right')
    else:
        raise AssertionError("awaited call did not return")
    assert events == ['receiver', 'left', 'middle', 'right', 'call']

    del receiver.collect
    events.clear()
    interrupted = scope['ordered']()
    assert interrupted.send(None) == 'paused'
    error = ValueError('interrupt')
    try:
        interrupted.throw(error)
    except ValueError as caught:
        assert caught is error
    else:
        raise AssertionError("throw was lost")
    assert events == ['receiver', 'left']

    events.clear()
    first = scope['first']()
    assert first.send(None) == 'paused'
    first.close()
    assert events == ['finally']

    outer = scope['outer']()
    assert outer.send(None) == 'paused'
    try:
        outer.send(receiver)
    except StopIteration as done:
        assert done.value is receiver
    else:
        raise AssertionError("outer await did not return")


def awaited_binary_operands():
    calls = []

    def right():
        calls.append('right')
        return Pause()

    class Reflected:
        def __rsub__(self, left):
            calls.append('subtract')
            return left

    async def difference():
        return (await Pause() - 1) - await right()

    coro = difference()
    assert coro.send(None) == 'paused'
    assert calls == []
    assert coro.send(42) == 'paused'
    assert calls == ['right']
    try:
        coro.send(Reflected())
    except StopIteration as done:
        assert done.value == 41
    else:
        raise AssertionError("awaited difference did not return")
    assert calls == ['right', 'subtract']


def awaited_comparisons():
    events = []

    class Truth:
        def __bool__(self):
            events.append('truth')
            return False

    result = Truth()

    class Left:
        def __ne__(self, right):
            events.append(('compare', right))
            return result

    def left():
        events.append('left')
        return Left()

    def right():
        events.append('right')
        return 0

    scope = {'left': left, 'right': right, 'pause': Pause()}
    exec("""\
async def branch():
    if left() != await pause:
        return 'first'
    elif await pause != right():
        return 'second'
    return 'neither'

async def value():
    return left() != await pause

async def loop():
    iterations = 0
    while await pause != 0:
        iterations += 1
        continue
    return iterations
""", scope)

    branch = scope['branch']()
    assert branch.send(None) == 'paused'
    assert events == ['left']
    assert branch.send(7) == 'paused'
    assert events == ['left', ('compare', 7), 'truth']
    try:
        branch.send(1)
    except StopIteration as done:
        assert done.value == 'second'
    else:
        raise AssertionError("awaited branch did not return")
    assert events == ['left', ('compare', 7), 'truth', 'right']

    events.clear()
    value = scope['value']()
    assert value.send(None) == 'paused'
    try:
        value.send(7)
    except StopIteration as done:
        assert done.value is result
    else:
        raise AssertionError("awaited comparison did not return")
    assert events == ['left', ('compare', 7)]

    events.clear()
    interrupted = scope['value']()
    assert interrupted.send(None) == 'paused'
    error = ValueError('interrupt')
    try:
        interrupted.throw(error)
    except ValueError as caught:
        assert caught is error
    else:
        raise AssertionError("throw was lost")
    assert events == ['left']

    loop = scope['loop']()
    assert loop.send(None) == 'paused'
    assert loop.send(7) == 'paused'
    try:
        loop.send(0)
    except StopIteration as done:
        assert done.value == 1
    else:
        raise AssertionError("awaited loop did not return")
