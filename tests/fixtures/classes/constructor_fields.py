"""Exercise the constructor shapes used by test.support._MemoryWatchdog."""


def observe(pid: int) -> int:
    print(pid)
    if pid < 0:
        raise ValueError("pid lookup failed")
    return pid


class Watch:
    def __init__(self, pid: int):
        self.procfile = "/proc/{pid}/statm".format(pid=observe(pid))
        self.started = False
        self.label = "ready"


class Missing:
    def __init__(self, pid: int):
        self.procfile = "{missing}".format(pid=observe(pid))


first = Watch(7)
second = Watch(21)
first.started = True
print(first.procfile)
print(first.started)
print(first.label)
print(second.procfile)
print(second.started)
try:
    Watch(-1)
except ValueError as exc:
    print(str(exc))
try:
    Missing(4)
except KeyError as exc:
    print(str(exc))
