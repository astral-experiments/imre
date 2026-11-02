import _signal
import signal


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
