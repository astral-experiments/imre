import gc
import types
import warnings
import weakref
from inspect import getcoroutinelocals


class Pause:
    def __await__(self):
        result = yield "paused"
        return result


def unawaited_source():
    async def job():
        return 37

    with warnings.catch_warnings(record=True) as seen:
        warnings.simplefilter("always", RuntimeWarning)
        live = job()
        released = []
        ref = weakref.ref(live, lambda _: released.append(len(seen)))
        job().close()
        gc.collect()
        assert not seen
        del live
        gc.collect()
        assert released == [0] and ref() is None
        assert len(seen) == 1
        assert seen[0].category is RuntimeWarning
        assert str(seen[0].message) == f"coroutine '{job.__qualname__}' was never awaited"
        assert completed_send(seen[0].source, None) == 37
        seen.clear()
        gc.collect()
        assert not seen


def current():
    unawaited_source()
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
    awaited_conditions()
    awaited_receiver()
    awaited_loop_iterable()


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

    events.clear()
    pending = scope['first']()
    assert pending.send(None) == 'paused'
    frame = pending.cr_frame
    reference = weakref.ref(pending)
    del pending
    gc.collect()
    assert reference() is None
    assert frame is not None and events == ['finally']
    del frame
    gc.collect()
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


def completed_send(task, value):
    try:
        task.send(value)
    except StopIteration as done:
        return done.value
    raise AssertionError('coroutine yielded instead of returning')


def awaited_conditions():
    events = []

    def pause(label):
        events.append(label)
        return Pause()

    class Verdict:
        def __bool__(self):
            events.append('truth')
            return events.count('truth') > 1

    verdict = Verdict()

    class Left:
        def __lt__(self, right):
            events.append('compare')
            return verdict

    async def choose():
        while await pause('wait') != 0:
            if not await pause('retry'):
                continue
            if Left() < await pause('middle') < pause('unreachable'):
                raise AssertionError('false comparison chain')
            return await pause('result') or pause('skipped')
        return None

    task = choose()
    assert task.send(None) == 'paused'
    assert task.send(1) == 'paused'
    assert task.send(False) == 'paused'
    assert task.send(2) == 'paused'
    assert task.send(True) == 'paused'
    assert task.send(3) == 'paused'
    assert completed_send(task, 42) == 42
    assert events == ['wait', 'retry', 'wait', 'retry', 'middle', 'compare', 'truth', 'result']

    async def value():
        return Left() < await pause('middle') < pause('skipped')

    events.clear()
    task = value()
    assert task.send(None) == 'paused'
    assert completed_send(task, 1) is verdict
    assert events == ['middle', 'compare', 'truth']

    async def mixed():
        return ((Left() < await pause('middle') < pause('skipped')) and 'end') or 'fallback'

    events.clear()
    task = mixed()
    assert task.send(None) == 'paused'
    assert completed_send(task, 1) == 'end'
    assert events == ['middle', 'compare', 'truth', 'truth']


    # A short-circuit in the fall-through arm can select the final result
    # directly. The other arm materializes its result before the enclosing and.
    async def tail(flag):
        return (None if flag else ((await Pause()) and 'inner')) and 'outer'

    async def head(flag):
        return (((await Pause()) and 'inner') if flag else None) and 'outer'

    for build, flag, expected, count in [(tail, False, verdict, 1), (head, True, 'outer', 2)]:
        events.clear()
        task = build(flag)
        assert task.send(None) == 'paused'
        result = completed_send(task, verdict)
        assert result is expected or result == expected
        assert events == ['truth'] * count

    async def skipped(flag):
        return (None if flag else (verdict and 'inner')) and await Pause()

    events.clear()
    assert completed_send(skipped(False), None) is verdict
    assert events == ['truth']


    # Long unparenthesized binary expressions use the same native stack budget as the
    # ordinary evaluator. The await at the left edge is discovered once during lowering.
    scope = {}
    exec("async def one(): return 1\nasync def long_sum(): return await one()" + " + 0" * 12000, scope)
    assert completed_send(scope['long_sum'](), None) == 1


def awaited_receiver():
    events = []

    class Process:
        @property
        def wait(self):
            events.append('lookup')
            return Pause

    async def status():
        return -await (await Pause()).wait()

    task = status()
    assert task.send(None) == 'paused'
    assert events == []
    assert task.send(Process()) == 'paused'
    assert events == ['lookup']
    assert completed_send(task, 7) == -7


def awaited_loop_iterable():
    events = []

    def labels():
        events.append("labels")
        return ("first", "second")

    async def rows():
        for key, value in zip(labels(), await Pause(), strict=False):
            events.append((key, value))
        else:
            events.append("done")

    task = rows()
    assert task.send(None) == "paused"
    assert events == ["labels"]
    assert completed_send(task, [7, 8, 9]) is None
    assert events == ["labels", ("first", 7), ("second", 8), "done"]
