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
