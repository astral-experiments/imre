from unittest import TestCase


class ParseError(Exception):
    pass


def parse(*, value):
    if value:
        raise ParseError(value)


def current():
    case = TestCase()
    raises = getattr(case, "assertRaises")
    error = ParseError("problem")
    with raises((ValueError, ParseError), msg=["--bad"]) as context:
        raise error
    assert context.exception is error
    assert error.__traceback__ is None

    try:
        with raises(ValueError):
            raise error
    except ParseError as got:
        assert got is error
    else:
        raise RuntimeError("unexpected exception was suppressed")

    try:
        with raises(ParseError, msg=["--bad"]):
            pass
    except AssertionError as missing:
        print(str(missing))
    else:
        raise RuntimeError("missing exception passed")

    # Holding a traceback must not retain inactive frames' locals after __exit__.
    from weakref import ref

    class Local:
        pass

    locals_seen = []
    tracebacks = []

    def leave_traceback():
        local = Local()
        locals_seen.append(ref(local))
        try:
            raise ParseError()
        except ParseError as caught:
            tracebacks.append(caught.__traceback__)
            raise

    with raises(ParseError):
        leave_traceback()
    try:
        with raises(ValueError):
            leave_traceback()
    except ParseError:
        pass
    assert len(tracebacks) == 2 and all(tracebacks)
    assert all(item() is None for item in locals_seen)

    class ShortMessage(TestCase):
        longMessage = False

    short = ShortMessage()
    assert TestCase.longMessage is True
    assert short.longMessage is False
    short.longMessage = True
    assert short.longMessage is True and ShortMessage().longMessage is False

    case.longMessage = False
    message = ["input"]
    try:
        with raises(ParseError, msg=message):
            pass
    except AssertionError as missing:
        assert missing.args[0] is message
    else:
        raise RuntimeError("short-message assertion passed")
    case.longMessage = True

    assert raises(ParseError, parse, value="bad") is None
    try:
        raises(ParseError, parse, value="")
    except AssertionError as missing:
        print(str(missing))
    else:
        raise RuntimeError("missing callable exception passed")

    try:
        with case.subTest(args=["--bad"]):
            case.fail("outside a running test")
    except AssertionError as exc:
        print(str(exc))
    else:
        raise RuntimeError("unattached subTest suppressed an exception")

    events = []

    def cleanup_error():
        raise ValueError("cleanup")

    def interrupt():
        raise KeyboardInterrupt("interrupt")

    case.addCleanup(events.append, "after error")
    case.addCleanup(cleanup_error)
    assert not case.doCleanups()
    assert events == ["after error"]
    case.addCleanup(events.append, "pending")
    case.addCleanup(interrupt)
    with raises(KeyboardInterrupt):
        case.doCleanups()
    assert events == ["after error"]
    assert case.doCleanups()
    assert events == ["after error", "pending"]
