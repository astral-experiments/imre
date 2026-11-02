import _signal
import struct

signal = __import__("".join(("sig", "nal")))


class Delay:
    def __index__(self):
        return 300


for member in (signal.SIGINT, signal.SIGALRM, signal.SIGTERM):
    assert member is signal.Signals(member.value)
    assert member == getattr(_signal, member.name)

assert signal.alarm is _signal.alarm
assert signal.alarm(Delay()) == 0
try:
    for args, kwargs in (([], {}), ([0, 0], {}), ([], {"seconds": 0}), ([1 << 40], {}), ([0.5], {})):
        try:
            signal.alarm(*args, **kwargs)
        except (TypeError, OverflowError) as error:
            print(type(error).__name__, error)
        else:
            raise AssertionError("invalid alarm arguments accepted")
finally:
    remaining = signal.alarm(0)
assert 0 < remaining <= 300
assert signal.alarm(0) == 0

assert signal.alarm(-1) == 0
remaining = signal.alarm(0)
if struct.calcsize("l") == 4:
    assert -(1 << 31) <= remaining < 0
else:
    assert (1 << 31) <= remaining <= (1 << 32) - 1


def current():
    assert tuple(signal.Handlers.__members__) == ("SIG_DFL", "SIG_IGN")
    assert list(signal.Handlers) == [signal.SIG_DFL, signal.SIG_IGN]
    for number, name in enumerate(("SIG_DFL", "SIG_IGN")):
        raw = getattr(_signal, name)
        assert type(raw) is int and raw == number
        member = getattr(signal, name)
        assert type(member) is signal.Handlers
        assert member is signal.Handlers(raw) is getattr(signal.Handlers, name)
        assert member.value == raw and member.name == name

    handler = signal.default_int_handler
    assert handler is _signal.default_int_handler
    assert type(handler) is type(len)
    assert handler.__name__ == handler.__qualname__ == "default_int_handler"
    assert handler.__self__ is _signal and handler.__module__ == "_signal"
    assert handler.__text_signature__ == "($module, signalnum, frame, /)"
    try:
        handler(Delay(), object())
    except KeyboardInterrupt as exc:
        assert exc.args == ()
    else:
        raise AssertionError("default handler returned")
    for args, kw in (
        ((), {}),
        ((1,), {}),
        ((1, 2, 3), {}),
        ((), {"signalnum": 2, "frame": None}),
        ((object(), None), {}),
        ((1 << 31, None), {}),
    ):
        try:
            handler(*args, **kw)
        except (TypeError, OverflowError) as exc:
            print(exc)
        else:
            raise AssertionError("invalid handler arguments accepted")
