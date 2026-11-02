import signal
import sys

# A blocked signal queued by the parent before exec remains pending in this process.
assert signal.SIGINT is signal.Signals(signal.SIGINT.value)
assert signal.SIGTERM is signal.Signals.SIGTERM
assert signal.SIGIOT is signal.SIGABRT
expected = int(sys.argv[1])
pending = signal.sigpending()
assert type(pending) is set
assert pending == ({expected} if expected else set()), "unexpected pending signals"
for number in pending:
    if number == signal.SIGRTMIN + 1:
        assert type(number) is int
    else:
        assert number is signal.Signals(expected)
print("verified pending signals", expected)
