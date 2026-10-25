import subprocess


def reject() -> int:
    raise ValueError("poll raised")


live = subprocess.Popen(["/bin/sleep", "30"])


class Probe:
    calls: int

    def __init__(self) -> None:
        self.calls = 0

    def change_status(self) -> None:
        self.calls += 1
        live.returncode = 23
        del live.poll


# The already-reaped child disappears between poll() and kill(). Restoring
# default polling makes the next kill observe ECHILD and cache zero.
finished = subprocess.Popen(["/bin/sh", "-c", "exit 1"])
while not finished.returncode:
    finished.poll()
finished.poll = lambda: None
finished.returncode = None
finished.send_signal(15)
print(finished.returncode is None)
del finished.poll
finished.kill()
print(finished.returncode == 0)
try:
    del finished.poll
except AttributeError:
    print("inherited poll")

probe = Probe()
try:
    live.poll = lambda: 2**90
    print(live.poll() == 2**90)
    print(live.returncode is None)
    # A different instance still polls its own child while this one is overridden.
    independent = subprocess.Popen(["/bin/sh", "-c", "exit 7"])
    print(independent.wait())
    print(independent.poll() == 7)
    # The callback's return value is ignored by send_signal.
    live.send_signal(0)
    live.poll = reject
    try:
        live.poll()
    except ValueError:
        print("direct poll raised")
    try:
        live.terminate()
    except ValueError as exc:
        print(str(exc))
    # The bound method mutates the actual child and removes its own override.
    live.poll = probe.change_status
    live.terminate()
    print(live.returncode == 23)
    print(probe.calls)
    live.poll = reject
    try:
        live.terminate()
    except ValueError:
        print("polled even with known status")
    del live.poll
    live.returncode = None
    print(live.poll() is None)
finally:
    live.returncode = None
    live.kill()
    print(live.wait())
