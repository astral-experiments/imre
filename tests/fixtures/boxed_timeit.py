import time
import timeit


def current():
    calls = []

    def setup():
        calls.append("setup")

    def work():
        calls.append("work")

    assert timeit.default_timer is time.perf_counter
    elapsed = timeit.Timer(work, setup).timeit(number=2)
    assert isinstance(elapsed, float) and elapsed >= 0
    assert calls == ["setup", "work", "work"]
    assert timeit.timeit(work, timer=time.process_time, number=1) >= 0
    assert calls == ["setup", "work", "work", "work"]


if __name__ == "__main__":
    current()
