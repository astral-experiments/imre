import types


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
    assert types.coroutine(suspend) is suspend
    marked_code = suspend.__code__
    assert old.gi_code is original_code and marked_code is not original_code
    assert types.coroutine(suspend) is suspend and suspend.__code__ is marked_code

    fresh = suspend(token)
    assert fresh.gi_code is marked_code
    relay = forward(fresh)
    assert relay.send(None) is token
    result = object()
    try:
        relay.send(result)
    except StopIteration as done:
        assert done.value is result
    else:
        raise AssertionError("decorated generator did not return")
    try:
        forward(old).send(None)
    except TypeError:
        pass
    else:
        raise AssertionError("decoration made an existing generator awaitable")
    finally:
        old.close()
