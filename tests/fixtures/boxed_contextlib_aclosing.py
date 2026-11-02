from contextlib import aclosing


def current():
    events = []
    body_error = ValueError("body")
    close_error = LookupError("close")

    class Resource:
        async def aclose(self):
            events.append("close")
            if self.fail:
                raise close_error

    resource = Resource()
    resource.fail = False

    async def use(fail_body):
        async with aclosing(resource) as entered:
            assert entered is resource
            events.append("body")
            if fail_body:
                raise body_error
            return 42

    for fail_body, fail_close, expected in (
        (False, False, None),
        (True, False, body_error),
        (True, True, close_error),
    ):
        resource.fail = fail_close
        coroutine = use(fail_body)
        try:
            coroutine.send(None)
        except StopIteration as done:
            assert expected is None and done.value == 42
        except (ValueError, LookupError) as raised:
            assert raised is expected
            if fail_close:
                assert raised.__context__ is body_error
        else:
            raise AssertionError("unexpected suspension")
        assert events == ["body", "close"]
        events.clear()
