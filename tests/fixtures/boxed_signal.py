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


def check_handlers():
    import faulthandler
    import gc
    import os

    seen = []

    def first(number, frame):
        seen.append((number, frame.f_code.co_name))

    def second(number, frame):
        raise RuntimeError("from second handler")

    if not faulthandler.is_enabled():
        previous = signal.signal(signal.SIGABRT, first)
        try:
            faulthandler.enable()
            assert signal.getsignal(signal.SIGABRT) is first
            # Simulated delivery uses the Python handler even while a native handler owns sigaction.
            from _thread import interrupt_main
            interrupt_main(signal.SIGABRT)
            assert seen.pop() == (signal.SIGABRT, "check_handlers")
            # map invokes the builtin twice within one call from Python.
            list(map(interrupt_main, (signal.SIGABRT, signal.SIGABRT)))
            assert seen == [(signal.SIGABRT, "check_handlers")]
            seen.clear()
            assert signal.signal(signal.SIGABRT, second) is first
            faulthandler.disable()
            assert signal.getsignal(signal.SIGABRT) is second
        finally:
            faulthandler.disable()
            signal.signal(signal.SIGABRT, previous)

    sig = signal.SIGUSR1
    original = signal.getsignal(sig)
    try:
        assert signal.signal(sig, first) is original
        gc.collect()
        assert signal.getsignal(sig) is first
        os.kill(os.getpid(), sig)
        assert seen == [(sig, "check_handlers")]
        assert signal.signal(sig, second) is first
        try:
            os.kill(os.getpid(), sig)
        except RuntimeError as exc:
            assert str(exc) == "from second handler"
        else:
            raise AssertionError("the handler exception did not reach the caller")
        assert signal.signal(sig, signal.SIG_IGN) is second
        os.kill(os.getpid(), sig)
        assert signal.getsignal(sig) is signal.SIG_IGN
        assert _signal.getsignal(sig) is _signal.SIG_IGN
        assert seen == [(sig, "check_handlers")]
        assert _signal.signal(sig, first) is _signal.SIG_IGN
        os.kill(os.getpid(), sig)
        assert seen == [(sig, "check_handlers"), (sig, "check_handlers")]
    finally:
        signal.signal(sig, original)
    assert signal.getsignal(sig) is original
    assert signal.getsignal(signal.SIGKILL) is signal.SIG_DFL
    assert _signal.getsignal(signal.SIGKILL) is _signal.SIG_DFL
    try:
        signal.signal(signal.SIGKILL, first)
    except OSError:
        pass
    else:
        raise AssertionError("SIGKILL cannot be caught")


check_handlers()
