def left(events: list[int], mark: int) -> list[int]:
    events.append(mark)
    return []


def right(events: list[int], mark: int) -> int:
    events.append(mark)
    return mark


def exercise(flag: bool) -> None:
    events: list[int] = []
    print(bool(left(events, saved := 1) if flag else right(events, saved := 2)))
    print(saved)
    print(not (left(events, saved := 3) if flag else right(events, saved := 4)))
    print(saved)
    if left(events, saved := 5) if flag else right(events, saved := 6):
        print("true")
    else:
        print("false")
    print(saved)
    for event in events:
        print(event)
    print(bool(([] if flag else 0) or events))


exercise(False)
exercise(True)
