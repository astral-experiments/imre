def stop_value(events: list[int], stop: int) -> int:
    events.append(1)
    print(f"stop {stop}")
    return stop


def unit_steps(stop: int, mode: int) -> None:
    events: list[int] = []
    target = -7
    try:
        for target in range(stop_value(events, stop)):
            try:
                print(f"unit {target}")
                if mode == 1:
                    break
                if mode == 2 and target == 2:
                    raise ValueError("body failure")
                stop = -9
                if target == 1:
                    continue
                target += 100
            finally:
                print(f"unit finally {target}")
        else:
            print("unit else")
    except ValueError as problem:
        print(str(problem))
    print(f"unit result {target} {stop} {len(events)}")


def larger_steps(stop: int, break_early: bool) -> None:
    events: list[int] = []
    target = -7
    for target in range(0, stop_value(events, stop), 3):
        try:
            print(f"larger {target}")
            if break_early:
                break
            stop = 0
            if target == 3:
                continue
            target = -target - 1
        finally:
            print(f"larger finally {target}")
    else:
        print("larger else")
    print(f"larger result {target} {stop} {len(events)}")


def failed_stop(events: list[int]) -> int:
    events.append(1)
    print("failed stop")
    raise ValueError("stop failure")


for stop in [-5, 0, 1, 2, 7]:
    unit_steps(stop, 0)
    larger_steps(stop, False)

# Probe both tagged-small guard thresholds without traversing enormous ranges.
maximum_small = (1 << 62) - 1
for stop in [
    maximum_small - 4,
    maximum_small - 3,
    maximum_small - 2,
    maximum_small - 1,
    maximum_small,
    maximum_small + 1,
    1 << 130,
    -(1 << 130),
]:
    unit_steps(stop, 1)
    larger_steps(stop, True)

unit_steps(4, 2)
events: list[int] = []
target = 99
try:
    for target in range(failed_stop(events)):
        print("unexpected body")
        break
    else:
        print("unexpected else")
except ValueError as problem:
    print(str(problem))
finally:
    print(f"stop finally {target} {len(events)}")
